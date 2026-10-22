//! バックグラウンドジョブワーカー（ADR-0024）。
//!
//! キャプチャ（録音）と重い処理（STT / 話者分離）を分離し、音声を正本にジョブ化して
//! **1 本ずつ直列**で処理する。キャプチャは `HEAVY_ML_JOB` permit を取らないので、重い処理の
//! 実行中でも新しい録音はいつでも始められる（並行録音）。重い 2 本同時＝16GB クラッシュ源は作らない
//! （ADR-0021）。永続キュー（`jobs` テーブル）なので、実行中にアプリが落ちても再起動で継続する。
//!
//! **設計の要（advisor）**: ワーカーループは *ジョブの内容では絶対に死なない*。1 本のジョブが
//! パニック（例: `spawn_blocking` 内の index out of bounds）しても core が Err を返しても、
//! どちらも `set_job_failed` に落としてループは次のジョブへ進む。ここで JoinError を握り潰さずに
//! 伝播させると、ワーカータスクごと死んで以後の全ジョブが pending のまま永久に止まる。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::path::{Path, PathBuf};

use mojiroku_core::store::{Job, SqliteStore, TITLE_JOB_KIND};
use tauri::{AppHandle, Manager};

use crate::commands::transcription::{cached_summary_model_path, run_local_llm, TITLE_MAX_TOKENS};
use crate::commands::{
    acquire_heavy_job_permit, core_err, emit_job_update, heavy_job_busy, job_progress_callback,
    resolve_models_dir, resolve_recordings_dir, JobUpdate,
};

/// 後付け話者分離で旧話者の表示名を新話者へ引き継ぐ声紋 cosine のしきい値。
/// 同一音声の再分離なので同一人物の声紋は高く出るはず。**保守的に**高めへ置き、確度の低い
/// 引き継ぎ（別人へ誤って名前が乗る）を避ける（ADR-0024。境界は実データで再調整しうる）。
const CARRY_DISPLAY_NAME_MIN_COS: f32 = 0.5;

/// ワーカーを起こす通知チャネル。`app.manage` して各 enqueue コマンドが `wake()` する。
/// `tokio::Notify` は「待機者がいなければ permit を 1 つ蓄える」ので、enqueue → wake が
/// ワーカーの `notified()` より先でも取りこぼさない（起こし損ねない）。
#[derive(Default)]
pub struct JobQueue {
    notify: tokio::sync::Notify,
    /// 実行中のジョブ id と中断フラグ（Issue #114）。`cancel_job` が立て、コアが見て止まる。
    running: std::sync::Mutex<Option<(String, Arc<AtomicBool>)>>,
}

/// 中断で止まったジョブの結果（エラーではなく canceled として記録する印）。
pub const JOB_CANCELED: &str = "error.job.canceled";

impl JobQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// 新しい pending を積んだら呼ぶ。眠っているワーカーを 1 回起こす。
    pub fn wake(&self) {
        self.notify.notify_one();
    }

    /// 実行中のジョブなら中断を求めて true。別のジョブ・実行中でなければ false。
    pub fn request_cancel(&self, job_id: &str) -> bool {
        match self.running.lock().unwrap().as_ref() {
            Some((id, flag)) if id == job_id => {
                flag.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }
}

/// ワーカーを起動する（`setup` 末尾で 1 回）。
/// 起動時に中断された running を pending へ戻し（再起動継続）、以後は pending を尽きるまで
/// 直列処理 → 空になれば `notified().await` で眠り、`wake()` で起きる。
pub fn spawn_worker(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // 起動時の復帰: 前回異常終了で running のまま残ったジョブを pending へ戻す。
        // 音声はディスクに確定しているので、頭から実行し直せば足りる。
        {
            let store = app.state::<SqliteStore>();
            match store.requeue_running_jobs() {
                Ok(n) if n > 0 => eprintln!("[jobs] 再起動継続: running {n} 本を pending へ復帰"),
                Ok(_) => {}
                Err(e) => eprintln!("[jobs] 起動時 requeue 失敗（新規ジョブは処理可）: {e}"),
            }
        }

        loop {
            // 次の pending を 1 本取る（State 借用は await をまたがせない）。
            let next = {
                let store = app.state::<SqliteStore>();
                store.next_pending_job()
            };
            match next {
                Ok(Some(job)) => run_one_job(&app, job).await,
                Ok(None) => {
                    // 空。wake() が来るまで眠る（蓄えられた permit があれば即起床）。
                    let queue = app.state::<JobQueue>();
                    queue.notify.notified().await;
                }
                Err(e) => {
                    // DB の一時エラーでワーカーを殺さない。少し待って継続する。
                    eprintln!("[jobs] next_pending_job 失敗、1s 後に再試行: {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
    });
}

/// ジョブ 1 本を実行する。**このループは決して panic を伝播させない**（ワーカーを殺さない）。
/// 成否は `set_job_done` / `set_job_failed` に必ず落とし、`job://update` で UI に通知する。
async fn run_one_job(app: &AppHandle, job: Job) {
    let job_id = job.id.clone();
    let recording_id = job.recording_id.clone();
    let kind = job.kind.clone();

    // running へ遷移して通知。
    {
        let store = app.state::<SqliteStore>();
        if let Err(e) = store.set_job_running(&job_id) {
            eprintln!("[jobs] set_job_running 失敗: {e}");
        }
    }
    emit_lifecycle(app, &job_id, &recording_id, &kind, "running", None, None);

    // 先行の重い処理があれば「順番待ち」を UI に出す（status は running のまま stage=queued）。
    if heavy_job_busy() {
        let store = app.state::<SqliteStore>();
        let _ = store.set_job_stage(&job_id, "queued");
        emit_lifecycle(
            app,
            &job_id,
            &recording_id,
            &kind,
            "running",
            Some("queued"),
            None,
        );
    }
    // 中断フラグを登録（順番待ちの間に押されても効くよう、permit を待つ前に）。
    let cancel = Arc::new(AtomicBool::new(false));
    *app.state::<JobQueue>().running.lock().unwrap() = Some((job_id.clone(), Arc::clone(&cancel)));

    // 重い ML を全体 1 本に直列化（既存セマフォ流用・ADR-0021）。permit は処理完了で手放す。
    let permit = acquire_heavy_job_permit().await;

    let result = if cancel.load(Ordering::Relaxed) {
        Err(JOB_CANCELED.to_string())
    } else {
        match kind.as_str() {
            "transcribe" => run_transcribe(app, &job, Arc::clone(&cancel)).await,
            "diarize" => run_diarize(app, &job, Arc::clone(&cancel)).await,
            TITLE_JOB_KIND => run_title(app, &job).await,
            other => Err(format!("error.job.unknown_kind: {other}")),
        }
    };
    drop(permit); // 重い区間はここまで（以降の判定/emit は軽い）
    *app.state::<JobQueue>().running.lock().unwrap() = None;

    // 結果を終端状態へ書き戻す（running のまま残さない＝再起動時の無限リトライを防ぐ）。
    let store = app.state::<SqliteStore>();
    match result {
        Ok(()) => {
            if let Err(e) = store.set_job_done(&job_id) {
                eprintln!("[jobs] set_job_done 失敗: {e}");
            }
            emit_lifecycle(app, &job_id, &recording_id, &kind, "done", None, None);
        }
        // 中断（Issue #114）。本文は書き換えていない（保存は処理の最後）ので、録音は元の状態のまま。
        Err(msg) if msg == JOB_CANCELED => {
            if let Err(e) = store.set_job_canceled(&job_id) {
                eprintln!("[jobs] set_job_canceled 失敗: {e}");
            }
            emit_lifecycle(app, &job_id, &recording_id, &kind, "canceled", None, None);
        }
        Err(msg) => {
            eprintln!("[jobs] ジョブ失敗 kind={kind} recording={recording_id}: {msg}");
            if let Err(e) = store.set_job_failed(&job_id, &msg) {
                eprintln!("[jobs] set_job_failed 失敗: {e}");
            }
            emit_lifecycle(
                app,
                &job_id,
                &recording_id,
                &kind,
                "failed",
                None,
                Some(&msg),
            );
        }
    }
}

/// ライフサイクル更新（進捗 done/total を伴わない status 遷移）を emit する薄いヘルパ。
fn emit_lifecycle(
    app: &AppHandle,
    job_id: &str,
    recording_id: &str,
    kind: &str,
    status: &str,
    stage: Option<&str>,
    error: Option<&str>,
) {
    emit_job_update(
        app,
        &JobUpdate {
            job_id: job_id.to_string(),
            recording_id: recording_id.to_string(),
            kind: kind.to_string(),
            status: status.to_string(),
            stage: stage.map(str::to_string),
            done: 0,
            total: None,
            error: error.map(str::to_string),
        },
    );
}

// ── 実処理 ────────────────────────────────────────────────────────────────────

/// 文字起こしジョブ。source_type に応じて音声トラックを解決し、STT（+ opt-in 話者分離）を回して
/// `replace_transcript` で本文を差し替える。会議は per-track が揃えばデュアルトラック。
async fn run_transcribe(app: &AppHandle, job: &Job, cancel: Arc<AtomicBool>) -> Result<(), String> {
    let id = job.recording_id.clone();
    let models_dir = resolve_models_dir(app)?;
    let rec_dir = resolve_recordings_dir(app)?;

    // source_type だけ先に読む（軽い。State 借用は await をまたがない）。
    // Also the meeting start offset (Issue #65); 0 when not stored (single track, old rows).
    let (source_type, mic_offset_ms) = {
        let store = app.state::<SqliteStore>();
        let source_type = store
            .get_recording_detail(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "error.recording.not_found".to_string())?
            .recording
            .source_type;
        let offset = store
            .get_mic_offset_ms(&id)
            .map_err(|e| e.to_string())?
            .unwrap_or(0);
        (source_type, offset)
    };

    let plan = resolve_transcribe_plan(&rec_dir, &id, source_type, job.params.diarize)?;
    let stt_lang = job.params.stt_lang.clone();
    let model = mojiroku_core::models::select_whisper_model(Some(&job.params.transcription_model));
    let lang = mojiroku_core::lang::Lang::from_code(&job.params.lang);
    let progress = job_progress_callback(app.clone(), job.id.clone(), id.clone(), job.kind.clone());

    // 重い core 呼び出しは spawn_blocking。panic は下の JoinError 分岐で failed に落とす。
    let cancel_for_worker = Arc::clone(&cancel);
    let handle = tauri::async_runtime::spawn_blocking(move || -> Result<
        (
            mojiroku_core::Transcript,
            Vec<mojiroku_core::Speaker>,
            Vec<mojiroku_core::diarization::SpeakerEmbedding>,
        ),
        String,
    > {
        let _cancel = mojiroku_core::cancel::scope(cancel_for_worker);
        let cb = progress;
        let options = mojiroku_core::TranscriptionOptions {
            language: stt_lang.as_deref(),
            model,
            ..Default::default()
        };
        match plan {
            TranscribePlan::DualTrack { mic, system } => mojiroku_core::transcribe_meeting_dual_track_with_options(
                &mic,
                &system,
                &models_dir,
                options,
                lang,
                mic_offset_ms,
                Some(&cb),
            )
            .map_err(core_err),
            TranscribePlan::Diarize(path) => mojiroku_core::transcribe_and_diarize_file_with_options(
                &path,
                &models_dir,
                options,
                lang,
                Some(&cb),
            )
            .map_err(core_err),
            TranscribePlan::SttOnly(path) => {
                mojiroku_core::transcribe_file_with_options(&path, &models_dir, options, Some(&cb))
                    .map(|t| (t, Vec::new(), Vec::new()))
                    .map_err(core_err)
            }
        }
    });
    let (transcript, speakers, embeddings) = match handle.await {
        Ok(r) => r?,
        Err(join) => return Err(format!("error.job.failed: {join}")),
    };

    // 中断が処理の最後の瞬間に来ていたら、書き込まない（Issue #114 レビュー）。
    if cancel.load(Ordering::Relaxed) {
        return Err(JOB_CANCELED.to_string());
    }
    // 本文・話者を差し替え（duration=0 の file はここで最終 segment 末尾に確定）。
    let store = app.state::<SqliteStore>();
    store
        .replace_transcript(&id, &transcript, &speakers)
        .map_err(|e| e.to_string())?;
    // 話者分離を含んだ場合は声紋も保存（ライブラリ照合・ADR-0018）。best-effort。
    if !embeddings.is_empty() {
        if let Err(e) = store.save_speaker_embeddings(
            &id,
            &embeddings,
            mojiroku_core::models::DEFAULT_DIAR_EMB_MODEL,
        ) {
            eprintln!("[jobs] 声紋の保存に失敗（本文は保存済み・照合のみ無効）: {e}");
        }
    }
    enqueue_auto_title(app, &store, job);
    Ok(())
}

/// 文字起こしが保存された録音に、タイトル自動生成ジョブを積む（Issue #4）。
/// 対象はマイク録音と会議で、タイトルが既定名のままのものだけ（`should_auto_title`）。
/// **要約モデルが手元に無ければ積まない**（数 GB のダウンロードを勝手に始めない）。
/// 自動生成は常にローカルで、クラウド設定でも文字起こしを外へ送らない。
/// ワーカーは今のジョブが終わると次の pending を取るので、`wake()` は要らない。
fn enqueue_auto_title(app: &AppHandle, store: &SqliteStore, job: &Job) {
    let wanted = match store.get_recording_detail(&job.recording_id) {
        Ok(Some(d)) => mojiroku_core::summarize::should_auto_title(&d.recording, &d.transcript),
        _ => false,
    };
    let has_model = resolve_models_dir(app)
        .and_then(|dir| cached_summary_model_path(app, &dir))
        .is_ok_and(|p| p.is_some());
    if !wanted || !has_model {
        return;
    }
    let id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = store.enqueue_job(&id, &job.recording_id, TITLE_JOB_KIND, &job.params) {
        eprintln!("[jobs] タイトル生成ジョブを積めなかった（録音はそのまま）: {e}");
    }
}

/// タイトル自動生成ジョブ（Issue #4）。**失敗しても録音には何も起きない**ので、生成できなかった
/// 場合はログだけ残して成功として終える（失敗の印を出すほどのことではない）。
/// 生成の前後でタイトルが既定名のままか確かめ直し、利用者が付けた名前は上書きしない。
async fn run_title(app: &AppHandle, job: &Job) -> Result<(), String> {
    let id = &job.recording_id;
    let transcript = {
        let store = app.state::<SqliteStore>();
        match store.get_recording_detail(id).map_err(|e| e.to_string())? {
            Some(d) if mojiroku_core::summarize::should_auto_title(&d.recording, &d.transcript) => {
                d.transcript
            }
            _ => return Ok(()),
        }
    };
    let models_dir = resolve_models_dir(app)?;
    let Some(model_path) = cached_summary_model_path(app, &models_dir)? else {
        return Ok(());
    };
    let lang = mojiroku_core::lang::Lang::from_code(&job.params.lang);
    let prompt = mojiroku_core::summarize::build_title_prompt(&transcript, lang);
    let raw = match run_local_llm(app, &model_path, &prompt, lang, Some(TITLE_MAX_TOKENS)).await {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!("[jobs] タイトル生成に失敗（既定名のまま）: {e}");
            return Ok(());
        }
    };
    let Some(title) = mojiroku_core::summarize::sanitize_title(&raw) else {
        eprintln!("[jobs] タイトルとして使える出力が無かった（既定名のまま）");
        return Ok(());
    };
    let store = app.state::<SqliteStore>();
    let still_default = store
        .get_recording_detail(id)
        .map_err(|e| e.to_string())?
        .is_some_and(|d| mojiroku_core::summarize::is_default_title(d.recording.title.as_deref()));
    if still_default {
        store
            .rename_recording(id, Some(&title))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 後付け（再）話者分離ジョブ。既存本文に新しい話者割当をマージし、旧表示名を声紋 cosine で
/// ベスト努力引き継ぎして `replace_speaker_assignments` で差し替える（要約は stale マーク）。
async fn run_diarize(app: &AppHandle, job: &Job, cancel: Arc<AtomicBool>) -> Result<(), String> {
    use mojiroku_core::SourceType;

    let id = job.recording_id.clone();
    let models_dir = resolve_models_dir(app)?;
    let rec_dir = resolve_recordings_dir(app)?;

    // 既存の本文・source_type・旧話者/声紋を読む（軽い。await をまたがない）。
    let (mut transcript, source_type, old_pairs, self_speaker, mic_offset_ms, had_speakers) = {
        let store = app.state::<SqliteStore>();
        let detail = store
            .get_recording_detail(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "error.recording.not_found".to_string())?;
        if detail.transcript.segments.is_empty() {
            // 文字起こし前は割当先が無い。先に transcribe すること。
            return Err("error.job.no_transcript".to_string());
        }
        let old_emb = store
            .get_speaker_embeddings(&id)
            .map_err(|e| e.to_string())?;
        // 旧 (Speaker, 声紋) ペア。声紋を持つ話者だけ（引き継ぎ元）。
        let old_pairs: Vec<(mojiroku_core::Speaker, Vec<f32>)> = detail
            .speakers
            .iter()
            .filter_map(|sp| {
                old_emb
                    .iter()
                    .find(|(sid, _)| sid == &sp.id)
                    .map(|(_, v)| (sp.clone(), v.clone()))
            })
            .collect();
        let self_speaker = detail
            .speakers
            .iter()
            .find(|sp| sp.id == mojiroku_core::merge::SELF_SPEAKER_ID)
            .cloned();
        let offset = store
            .get_mic_offset_ms(&id)
            .map_err(|e| e.to_string())?
            .unwrap_or(0);
        let had_speakers = detail
            .speakers
            .iter()
            .any(|sp| sp.id != mojiroku_core::merge::SELF_SPEAKER_ID);
        (
            detail.transcript,
            detail.recording.source_type,
            old_pairs,
            self_speaker,
            offset,
            had_speakers,
        )
    };

    // 対象音声を解決。会議（Live）は **system（相手）トラックだけ**を分離し直す（Issue #102）。
    // mic（自分）のセグメントは `self` のまま触らない。以前は全 transcript へ merge すると自分の
    // セグメントが時間重なりで相手話者へ化けるため拒否していたが、`self` を保てば安全に再分離できる。
    let audio = match source_type {
        SourceType::Live => {
            let system = rec_dir.join(format!("{id}-system.wav"));
            if !system.exists() {
                return Err("error.job.no_pertrack".to_string());
            }
            system
        }
        SourceType::Mic | SourceType::File => {
            find_primary_audio(&rec_dir, &id).ok_or_else(|| "error.job.no_audio".to_string())?
        }
    };

    let lang = mojiroku_core::lang::Lang::from_code(&job.params.lang);
    let progress = job_progress_callback(app.clone(), job.id.clone(), id.clone(), job.kind.clone());

    let cancel_for_worker = Arc::clone(&cancel);
    let handle = tauri::async_runtime::spawn_blocking(
        move || -> Result<mojiroku_core::diarization::DiarizationResult, String> {
            let _cancel = mojiroku_core::cancel::scope(cancel_for_worker);
            let cb = progress;
            mojiroku_core::diarize_file(&audio, &models_dir, lang, Some(&cb)).map_err(core_err)
        },
    );
    let diar = match handle.await {
        Ok(r) => r?,
        Err(join) => return Err(format!("error.job.failed: {join}")),
    };
    // 話者分離の直後に来た中断は、話者の割り当てを書き換える前にここで止める（Issue #114 レビュー）。
    if cancel.load(Ordering::Relaxed) {
        return Err(JOB_CANCELED.to_string());
    }

    // A re-run that finds no speech turns would erase every existing speaker. Keep the
    // current assignments and report it instead (Issue #102).
    if diar.turns.is_empty() && had_speakers {
        return Err("error.job.no_speakers_found".to_string());
    }

    // 本文へ新話者割当をマージ（純関数・text 不変）。
    let speakers = if source_type == SourceType::Live {
        // merge_tracks は開始が早かった側を後ろへずらして保存している。system 側がずれたのは
        // mic_offset_ms が負のとき（Issue #65）。
        let system_shift_ms = if mic_offset_ms < 0 {
            mic_offset_ms.unsigned_abs()
        } else {
            0
        };
        mojiroku_core::merge::reassign_meeting_speakers(
            &mut transcript,
            &diar,
            system_shift_ms,
            self_speaker,
            lang,
        )
    } else {
        // From scratch, so labels from an older diarization do not survive (Issue #102).
        mojiroku_core::merge::reassign_speakers(&mut transcript, &diar);
        diar.speakers.clone()
    };

    // 新 (Speaker, 声紋) ペア → 旧表示名の引き継ぎ remap。
    let new_pairs: Vec<(mojiroku_core::Speaker, Vec<f32>)> = diar
        .speakers
        .iter()
        .filter_map(|sp| {
            diar.embeddings
                .iter()
                .find(|e| e.speaker_id == sp.id)
                .map(|e| (sp.clone(), e.vector.clone()))
        })
        .collect();
    let remap = mojiroku_core::diarization::carry_display_names(
        &old_pairs,
        &new_pairs,
        CARRY_DISPLAY_NAME_MIN_COS,
    );

    let store = app.state::<SqliteStore>();
    store
        .replace_speaker_assignments(
            &id,
            &transcript,
            &speakers,
            &diar.embeddings,
            mojiroku_core::models::DEFAULT_DIAR_EMB_MODEL,
            &remap,
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 文字起こし対象トラックの構成。
enum TranscribePlan {
    /// 会議のデュアルトラック（mic=自分・system=相手）。
    DualTrack { mic: PathBuf, system: PathBuf },
    /// 単一音声を STT + 話者分離。
    Diarize(PathBuf),
    /// 単一音声を STT のみ。
    SttOnly(PathBuf),
}

/// source_type と存在する WAV から文字起こし構成を決める。
/// 会議は per-track の有無で dual/相手のみ/自分のみを判定（`stop_meeting_recording` と同じ規約）。
fn resolve_transcribe_plan(
    rec_dir: &Path,
    id: &str,
    source_type: mojiroku_core::SourceType,
    diarize: bool,
) -> Result<TranscribePlan, String> {
    use mojiroku_core::SourceType;
    match source_type {
        SourceType::Live => {
            let mic = rec_dir.join(format!("{id}-mic.wav"));
            let system = rec_dir.join(format!("{id}-system.wav"));
            match (mic.exists(), system.exists()) {
                (true, true) => Ok(TranscribePlan::DualTrack { mic, system }),
                // 相手のみ = 相手音声を STT + 話者分離（複数話者を分離）。
                (false, true) => Ok(TranscribePlan::Diarize(system)),
                // 自分のみ = マイク相当の単一話者 STT。
                (true, false) => Ok(TranscribePlan::SttOnly(mic)),
                (false, false) => Err("error.job.no_pertrack".to_string()),
            }
        }
        SourceType::Mic | SourceType::File => {
            let path =
                find_primary_audio(rec_dir, id).ok_or_else(|| "error.job.no_audio".to_string())?;
            if diarize {
                Ok(TranscribePlan::Diarize(path))
            } else {
                Ok(TranscribePlan::SttOnly(path))
            }
        }
    }
}

/// recordings/ から録音 id の主音声（`<id>.<ext>`）を探す。会議の per-track（`<id>-mic`/`-system`）は
/// file_stem が異なるので拾わない（`recording_audio_src` と同じ規約）。無ければ None。
fn find_primary_audio(rec_dir: &Path, id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(rec_dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.file_stem().and_then(|s| s.to_str()) == Some(id) {
            return Some(p);
        }
    }
    None
}
