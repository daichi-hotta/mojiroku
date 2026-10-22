//! Tauri コマンド層。`lib.rs` の `run()` が `generate_handler!` で登録する。
//!
//! 重い処理は `spawn_blocking` で回し、`State` は `spawn_blocking` の外で扱う
//! （`mojiroku-core` は UI 非依存）。本モジュールは複数コマンドで共有する型・ヘルパーを持ち、
//! 各コマンドはドメイン別サブモジュール（transcription / recording / history / speaker /
//! export / settings）に分割している。

pub mod export;
pub mod history;
pub mod jobs;
pub mod recording;
pub mod settings;
pub mod speaker;
pub mod transcription;

use crate::secrets;
use mojiroku_core::store::SqliteStore;
use tauri::{AppHandle, Emitter, Manager};

/// 進捗イベントのペイロード（`transcribe://progress`）。
#[derive(Clone, serde::Serialize)]
struct Progress {
    stage: String,
    done: u64,
    total: Option<u64>,
}

/// 録音停止・ファイル取り込み・ジョブ投入系コマンドの共通の戻り（ADR-0024 非同期フリップ）。
/// キャプチャ（音声確定）は同期で終わり、STT/話者分離はワーカーへ委譲して**即座に返す**。
/// フロントは即座に DetailView へ遷移し、`job://update`（`job_id`/`recording_id`）で進捗を追う。
#[derive(Clone, serde::Serialize)]
pub(crate) struct StartJobResult {
    pub recording_id: String,
    /// 投入したジョブ ID。**録音のみ保存（ADR-0024 増分5）ではジョブを作らない**ので None
    /// （フロントは録音行だけ作られたと解釈し、DetailView の「文字起こしを実行」導線へ委ねる）。
    pub job_id: Option<String>,
}

/// バックグラウンドジョブの更新イベント（`job://update`）。ライフサイクル遷移（status）も
/// ステージ進捗（stage/done/total）も 1 ペイロードに相乗りさせる。**`job_id`/`recording_id` を必ず
/// 載せる**のが要点で、複数の詳細ビュー・履歴行が自分宛の更新だけを描けるようにする（相関付け）。
#[derive(Clone, serde::Serialize)]
pub(crate) struct JobUpdate {
    pub job_id: String,
    pub recording_id: String,
    pub kind: String,
    /// "pending" | "running" | "done" | "failed" | "canceled"。
    pub status: String,
    /// 実行中の処理ステージ（decode/transcribe/diarization/merge/queued 等）。ライフサイクルのみの
    /// 更新（done/failed）では None。
    pub stage: Option<String>,
    pub done: u64,
    pub total: Option<u64>,
    /// 失敗時のキー化メッセージ（`translateError` 対象）。それ以外は None。
    pub error: Option<String>,
    /// 話者分離のやり直しで引き継げなかった手動訂正の数（done のときだけ。ADR-0048）。
    pub unmapped_corrections: Option<usize>,
}

/// `job://update` を emit する薄いヘルパ。失敗は無視（進捗表示は best-effort）。
pub(crate) fn emit_job_update(app: &AppHandle, update: &JobUpdate) {
    let _ = app.emit("job://update", update);
}

// ── 重い ML ジョブの直列化 ──────────────────────────────────────────────────

/// 重い ML ジョブ（whisper STT / sherpa 話者分離 / ローカル LLM 要約 sidecar）を
/// アプリ全体で**同時 1 本**に直列化するセマフォ。
///
/// 16GB 級のマシンで whisper(Metal) + onnxruntime + llama sidecar(4.4GB) が同時に走ると
/// メモリ枯渇で C++ 例外（std::bad_alloc）→ v0.3.0 ではプロセス abort（クラッシュ）、
/// 良くてもスワップでシステムごと停滞（フリーズ）していた（docs/error.md の実クラッシュが動機。
/// 例外自体は core の ffi_guard が Err 化するが、そもそも枯渇させないのが本命）。
static HEAVY_ML_JOB: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// 重い ML ジョブが実行中か（live STT の tick スキップ等、soft な譲り合い判定用）。
pub(crate) fn try_acquire_live_job() -> Option<tokio::sync::SemaphorePermit<'static>> {
    HEAVY_ML_JOB.try_acquire().ok()
}

pub(crate) fn heavy_job_busy() -> bool {
    HEAVY_ML_JOB.available_permits() == 0
}

/// 重い ML ジョブの実行権を取得する（解放は戻り値 permit の drop）。
/// 先行ジョブがある間は完了まで待ち、待ちに入ることを `event` の stage="queued" で UI に通知する。
pub(crate) async fn acquire_heavy_job(
    app: &AppHandle,
    event: &str,
) -> tokio::sync::SemaphorePermit<'static> {
    if heavy_job_busy() {
        emit_progress(app, event, "queued", 0, None);
    }
    HEAVY_ML_JOB
        .acquire()
        .await
        .expect("HEAVY_ML_JOB semaphore は close しない")
}

/// 重い ML ジョブの実行権をイベント通知なしで取得する（バックグラウンドワーカー用）。
/// 待ちに入ったことの UI 通知は呼び出し側が `job://update`（stage="queued"）で行うため、
/// ここでは permit を待って返すだけにする（`acquire_heavy_job` の event 版と役割分担）。
pub(crate) async fn acquire_heavy_job_permit() -> tokio::sync::SemaphorePermit<'static> {
    HEAVY_ML_JOB
        .acquire()
        .await
        .expect("HEAVY_ML_JOB semaphore は close しない")
}

// ── 共通ヘルパ（複数コマンドで重複していた定型を集約） ──────────────────────────

/// core の Err をフロント向け文字列へ変換する。
///
/// 主要な user-facing エラーは `error.<domain>.<cause>[: 詳細]` の安定キーで表し、フロントの
/// `translateError`（`frontend/src/i18n/index.tsx`）がアプリ言語の文言へ置き換える（未知キーは
/// 原文フォールバック）。core 側でキー化済みのメッセージは `CoreError` の Display 接頭辞
/// （"model error: " 等）を外し、**キーが文字列の先頭に来る**ようにする（translateError は
/// 先頭のキーしか解釈しない）。それ以外は従来どおり Display 文字列を返す。
///
/// **対象バリアントは match の arm がすべて。** キー化したエラーを新しいバリアントで
/// 返すときは、ここに足さないと接頭辞が付いたまま UI に出る（Issue #19 で Db を追加）。
pub(crate) fn core_err(e: mojiroku_core::CoreError) -> String {
    use mojiroku_core::CoreError;
    match e {
        CoreError::Cancelled => crate::jobs::JOB_CANCELED.to_string(),
        CoreError::Model(m) | CoreError::Calendar(m) | CoreError::Db(m)
            if m.starts_with("error.") =>
        {
            m
        }
        other => other.to_string(),
    }
}

/// `app_data_dir` を解決する（複数コマンドで重複していた app_data_dir + map_err を集約）。
pub(crate) fn resolve_app_data_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
}

/// モデル保存先 `app_data_dir/models` を解決する。
pub(crate) fn resolve_models_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    resolve_app_data_dir(app).map(|d| d.join("models"))
}

/// 録音原本の保存先 `app_data_dir/recordings` を解決する
/// （録音再生パス解決・原本コピー・削除・履歴で重複していた `join("recordings")` を集約）。
pub(crate) fn resolve_recordings_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    resolve_app_data_dir(app).map(|d| d.join("recordings"))
}

/// アプリ設定を読む（`app_data_dir` 解決 → `settings.json` ロード。無ければ既定）。
/// 設定・要約・エクスポートで重複していた `settings::load(&resolve_app_data_dir(app)?)` を集約。
pub(crate) fn load_settings(app: &AppHandle) -> Result<crate::settings::Settings, String> {
    Ok(crate::settings::load(&resolve_app_data_dir(app)?))
}

/// 録音 id から owned な `RecordingDetail` を取り出す（無ければ `error.recording.not_found`）。
/// Notion/Slack エクスポートで重複していた取得 + not-found 検証を集約。
pub(crate) fn resolve_recording_detail(
    store: &SqliteStore,
    id: &str,
) -> Result<mojiroku_core::store::RecordingDetail, String> {
    store
        .get_recording_detail(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "error.recording.not_found".to_string())
}

/// 録音タイトルを確定する（trim して空なら `default`）。Mic/会議 保存で重複していた
/// タイトル・フォールバックを集約。
pub(crate) fn resolve_recording_title(title: Option<String>, default: &str) -> String {
    title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// キーチェーンから必須シークレットを取り出す（空白のみは未設定扱い）。未設定なら `missing_msg`。
/// Notion トークン / Slack webhook / iCal URL / BYOK API キーの取得で重複していた検証を集約。
pub(crate) fn get_secret_or_error(name: &str, missing_msg: &str) -> Result<String, String> {
    secrets::get(name)?
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| missing_msg.to_string())
}

/// 進捗イベントを emit する薄いヘルパ（`Progress` 構築の重複を畳む）。
/// 失敗は無視する（UI 進捗表示は best-effort）。
pub(crate) fn emit_progress(
    app: &AppHandle,
    event: &str,
    stage: &str,
    done: u64,
    total: Option<u64>,
) {
    let _ = app.emit(
        event,
        Progress {
            stage: stage.to_string(),
            done,
            total,
        },
    );
}

/// 録音行（transcript 無し）を作り、`record_only` でなければ文字起こしジョブを積む共通ヘルパ
/// （非同期フリップ・ADR-0024）。`record_only=true`（増分5「音声だけ保存」）は録音行だけ作って
/// ジョブは積まない → 後で DetailView の「文字起こしを実行」（`transcribe_recording`）から処理する。
/// **順序が load-bearing**: `insert_recording_only`（recordings 行を commit）→ `enqueue_job`
/// （jobs 行を commit。FK が recordings を参照）→ `queue.wake()`。enqueue を先にすると、ワーカーが
/// recordings 行のできる前にジョブを拾って `error.recording.not_found` を踏むレースになる。
pub(crate) fn insert_recording_and_maybe_enqueue(
    app: &AppHandle,
    store: &SqliteStore,
    queue: &crate::jobs::JobQueue,
    recording: &mojiroku_core::Recording,
    diarize: bool,
    record_only: bool,
    // Meeting recordings only (Issue #65): mic-vs-system start offset. Written before the
    // job is enqueued because the worker may pick the job up immediately.
    mic_offset_ms: Option<i64>,
) -> Result<StartJobResult, String> {
    insert_recording_and_maybe_enqueue_with_translations(
        app,
        store,
        queue,
        recording,
        diarize,
        record_only,
        mic_offset_ms,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_recording_and_maybe_enqueue_with_translations(
    app: &AppHandle,
    store: &SqliteStore,
    queue: &crate::jobs::JobQueue,
    recording: &mojiroku_core::Recording,
    diarize: bool,
    record_only: bool,
    mic_offset_ms: Option<i64>,
    translations: &[mojiroku_core::store::SavedLiveTranslation],
) -> Result<StartJobResult, String> {
    store
        .insert_recording_with_translations(recording, translations)
        .map_err(|e| e.to_string())?;
    if let Some(offset) = mic_offset_ms {
        store
            .set_mic_offset_ms(&recording.id, offset)
            .map_err(|e| e.to_string())?;
    }
    let job_id = if record_only {
        None
    } else {
        // params（言語・話者分離指定）は enqueue 時にスナップショット。録音のみ保存では処理を後回しに
        // するため、話者分離指定は record 時に確定せず後の transcribe_recording 時に選び直す。
        let params = jobs::snapshot_params(app, diarize)?;
        let jid = uuid::Uuid::new_v4().to_string();
        store
            .enqueue_job(&jid, &recording.id, "transcribe", &params)
            .map_err(|e| e.to_string())?;
        queue.wake();
        Some(jid)
    };
    Ok(StartJobResult {
        recording_id: recording.id.clone(),
        job_id,
    })
}

/// バックグラウンドジョブの進捗を `job://update` へ流すコールバックを作る。
/// **job_id/recording_id を載せる**ので、複数ジョブが走っても UI が自分宛だけ描ける（要石: 進捗の相関付け）。
///
/// DB の `jobs.stage` は**ステージ名が変わったときだけ**書く（モデル DL のバイト tick で毎回 UPDATE
/// しない）。イベント（done/total 付き）は毎 tick 発火してよい（best-effort・軽量）。
/// `RefCell` で直近ステージを持つので closure は `Fn`（`spawn_blocking` へ持ち込める Send）を保つ。
pub(crate) fn job_progress_callback(
    app: AppHandle,
    job_id: String,
    recording_id: String,
    kind: String,
) -> impl Fn(&str, u64, Option<u64>) {
    let last_stage: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
    move |stage: &str, done: u64, total: Option<u64>| {
        let changed = last_stage.borrow().as_deref() != Some(stage);
        if changed {
            *last_stage.borrow_mut() = Some(stage.to_string());
            let store = app.state::<SqliteStore>();
            if let Err(e) = store.set_job_stage(&job_id, stage) {
                eprintln!("[jobs] set_job_stage 失敗（進捗表示のみ・処理は続行）: {e}");
            }
        }
        emit_job_update(
            &app,
            &JobUpdate {
                job_id: job_id.clone(),
                recording_id: recording_id.clone(),
                kind: kind.clone(),
                status: "running".to_string(),
                stage: Some(stage.to_string()),
                done,
                total,
                error: None,
                unmapped_corrections: None,
            },
        );
    }
}

/// 取り込んだ音声ファイルの原本を `recordings/<id>.<元拡張子>` へ複製する。
/// **フリップ後（ADR-0024）はワーカーがこのコピーを STT の入力に読む正本**なので、成功は必須
/// （旧 best-effort から致命へ格上げ。失敗したら呼び出し側は録音行もジョブも作らない）。
/// 音声確定を返答前に済ませる ADR-0023 の不変条件（rename ゲート）を File 経路にも揃える。
/// 大きいファイルのコピーで async executor を塞がないよう `spawn_blocking` で行う。
pub(crate) async fn copy_recording_original(
    app: &AppHandle,
    id: &str,
    src_path: &str,
) -> Result<(), String> {
    let rec_dir = resolve_recordings_dir(app)?;
    let ext = std::path::Path::new(src_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin")
        .to_string();
    let src = src_path.to_string();
    let dest = rec_dir.join(format!("{id}.{ext}"));
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&rec_dir).and_then(|_| std::fs::copy(&src, &dest).map(|_| ()))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("error.recording.copy_failed: {e}"))
}

/// PCM(f32 interleaved, [-1,1]) を 16bit int WAV に書き出すワンショットヘルパ。
/// 本番の録音経路は spool 化（ADR-0023, `audio::spool::WavSpoolWriter` へ逐次書き）で
/// ここを通らなくなったため、旧経路との等価性テスト・量子化の回帰テスト用に残す。
#[cfg(test)]
pub(crate) fn write_wav(
    path: &std::path::Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<(), String> {
    let mut writer = crate::audio::spool::WavSpoolWriter::create(path, sample_rate, channels)?;
    writer.append(samples)?;
    writer.finalize().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// write_wav: [-1,1] クランプと i16 量子化を WAV へ書き出し、読み戻して検証する。
    /// 範囲外（±2.0）は ±i16::MAX に丸まり、0.5 は floor 量子化される。
    #[test]
    fn write_wav_clamps_and_roundtrips() {
        let samples = [0.0f32, 1.0, -1.0, 2.0, -2.0, 0.5];
        let path = std::env::temp_dir().join(format!(
            "mojiroku-write-wav-test-{}.wav",
            std::process::id()
        ));
        write_wav(&path, &samples, 16_000, 1).unwrap();

        let mut reader = hound::WavReader::open(&path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 16_000);
        assert_eq!(spec.bits_per_sample, 16);
        let got: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
        let max = i16::MAX;
        let half = (0.5_f32 * max as f32) as i16; // floor 量子化（16383）
        assert_eq!(got, vec![0, max, -max, max, -max, half]);

        let _ = std::fs::remove_file(&path);
    }

    /// get_secret_or_error: 存在しない account 名は未設定として `missing_msg` を返す
    /// （空文字/未登録ガード。debug ビルドは dev 平文ストアを read-only で参照するだけ）。
    #[test]
    fn get_secret_or_error_reports_missing() {
        let r = get_secret_or_error("__mojiroku_absent_secret_for_test__", "未設定です");
        assert_eq!(r, Err("未設定です".to_string()));
    }

    /// core_err: `error.*` キーで始まる core メッセージは Display 接頭辞（"model error: " 等）を
    /// 外してキー先頭のまま返し、それ以外は従来どおり Display 文字列を返す。
    #[test]
    fn core_err_strips_display_prefix_only_for_keys() {
        let keyed = mojiroku_core::CoreError::Model("error.model.download: req: timeout".into());
        assert_eq!(core_err(keyed), "error.model.download: req: timeout");

        let keyed_cal = mojiroku_core::CoreError::Calendar("error.calendar.not_connected".into());
        assert_eq!(core_err(keyed_cal), "error.calendar.not_connected");

        let plain = mojiroku_core::CoreError::Model("notion json: bad".into());
        assert_eq!(core_err(plain), "model error: notion json: bad");

        let io = mojiroku_core::CoreError::Io("error.model.download: fake".into());
        assert_eq!(core_err(io), "io error: error.model.download: fake");

        // Db も対象（発言単位の話者訂正が返すキー・Issue #19）。接頭辞が付くと
        // translateError の辞書に当たらず、日本語 UI に英語が素通しで出る。
        let db = mojiroku_core::CoreError::Db("error.segment.not_found".into());
        assert_eq!(core_err(db), "error.segment.not_found");
        let db_plain = mojiroku_core::CoreError::Db("boom".into());
        assert_eq!(core_err(db_plain), "db error: boom");
    }
}
