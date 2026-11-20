//! 録音コマンド（再生用パス解決、マイク録音、会議モードのデュアルトラック録音）。
//!
//! spool 化（ADR-0023）: キャプチャ中の PCM は `recordings/.spool/<uuid>-{mic,system}.wav`
//! へ逐次書き出され、停止時に正式名（`<id>.wav` / `<id>-mic.wav` / `<id>-system.wav`）へ
//! rename して確定する。`.spool` を recordings/ 配下に置くのは **rename の同一ボリューム保証**
//! のため（tmp に移すなら copy フォールバックが必要）。クラッシュ残骸は起動時に掃除（lib.rs）。

use std::path::PathBuf;

use super::*;
use crate::jobs::JobQueue;
use crate::{live_stt, mic, system_audio};
use tauri::State;

/// spool ディレクトリ（recordings/.spool）を確保して返す。
fn resolve_spool_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = resolve_recordings_dir(app)?.join(".spool");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// 録音 id に対応する再生可能な音声ファイルの絶対パスを返す（無ければ None）。
/// File=<id>.<元拡張子> / Mic=<id>.wav / 会議=<id>.wav（結合ミックス）。recordings/ を走査し
/// file_stem が id に一致する 1 本（`-mic`/`-system` 等の枝は file_stem が異なるので自然に除外）を
/// 選ぶ。フロントは convertFileSrc でアセット URL 化して <audio> 再生する（assetProtocol scope 要）。
#[tauri::command]
pub(crate) fn recording_audio_src(app: AppHandle, id: String) -> Result<Option<String>, String> {
    let rec_dir = resolve_recordings_dir(&app)?;
    let entries = match std::fs::read_dir(&rec_dir) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.file_stem().and_then(|s| s.to_str()) == Some(id.as_str()) {
            return Ok(Some(p.to_string_lossy().to_string()));
        }
    }
    Ok(None)
}

/// マイク録音開始。PCM は spool WAV へ逐次書き出される（全量 RAM 蓄積をしない）。
#[tauri::command]
pub(crate) fn start_mic_recording(
    app: AppHandle,
    mic: State<'_, mic::MicState>,
) -> Result<(), String> {
    let spool = resolve_spool_dir(&app)?.join(format!("{}-mic.wav", uuid::Uuid::new_v4()));
    mic::start(&mic, spool)
}

/// マイク録音を破棄停止（保存しない・Issue #113）。spool WAV も消す。
#[tauri::command]
pub(crate) fn cancel_mic_recording(mic: State<'_, mic::MicState>) -> Result<(), String> {
    let info = mic::stop(&mic)?;
    // 消せなかったら破棄できたと言わない（録音は止まっているが、音声が残っている）。
    // 残った spool は次回起動時の掃除でも消える（lib.rs）。
    match std::fs::remove_file(&info.spool_path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("error.recording.discard_failed: {e}")),
    }
}

/// 録音中の入力音量（前回の呼び出し以降の最大振幅 0.0〜1.0・Issue #113）。
/// 録音していないトラックは None。録音画面が一定間隔で読み、実際の音量として表示する
/// （以前の波形は飾りで、マイクが音を拾っているか分からなかった）。
#[derive(serde::Serialize)]
pub(crate) struct RecordingLevels {
    mic: Option<f32>,
    system: Option<f32>,
}

#[tauri::command]
pub(crate) fn recording_levels(
    mic: State<'_, mic::MicState>,
    sys: State<'_, system_audio::SystemAudioState>,
) -> RecordingLevels {
    RecordingLevels {
        mic: mic::live_handle(&mic).map(|(pcm, _, _)| pcm.take_peak()),
        system: system_audio::live_handle(&sys).map(|(pcm, _)| pcm.take_peak()),
    }
}

/// マイク録音停止 → spool WAV を正式名へ rename → 文字起こしジョブを投入して**即座に返す**
/// （ADR-0024 非同期フリップ）。音声確定（rename）は同期のまま、STT はワーカーへ委譲する。
#[tauri::command]
pub(crate) async fn stop_mic_recording(
    app: AppHandle,
    store: State<'_, SqliteStore>,
    mic: State<'_, mic::MicState>,
    queue: State<'_, JobQueue>,
    diarize: bool,
    // 「記録を準備」（カレンダー連携）から渡される予定タイトル。未指定/空なら既定の「録音」。
    title: Option<String>,
    // true（増分5「音声だけ保存」）なら録音行だけ作ってジョブは積まない（後で文字起こし）。
    record_only: bool,
) -> Result<StartJobResult, String> {
    // 1) 停止（spool WAV が finalize される。join 後に受けるので競合なし）。
    let info = mic::stop(&mic)?;
    if info.samples_written == 0 {
        let _ = std::fs::remove_file(&info.spool_path);
        return Err("error.recording.empty".into());
    }
    if let Some(e) = &info.spool_error {
        eprintln!("マイク録音の書き出しで一部エラー（部分保存で続行）: {e}");
    }
    let duration_ms = mic::duration_ms(
        info.samples_written as usize,
        info.channels,
        info.sample_rate,
    );
    let sample_rate = info.sample_rate;

    // 2) recordings/<id>.wav へ rename して確定（id は Recording.id と共用。同一ボリューム）。
    //    音声を返答前にディスクへ確定させる（ADR-0023 の不変条件。ワーカーはこの WAV を STT で読む）。
    let id = uuid::Uuid::new_v4().to_string();
    let rec_dir = resolve_recordings_dir(&app)?;
    std::fs::create_dir_all(&rec_dir).map_err(|e| e.to_string())?;
    let wav_path = rec_dir.join(format!("{id}.wav"));
    std::fs::rename(&info.spool_path, &wav_path).map_err(|e| e.to_string())?;

    // 3) Mic 録音行（transcript 無し）を作り、文字起こしジョブを積む。予定タイトル（カレンダー連携）が
    //    あれば使い、無ければ既定の「録音」/ "Recording"。タイトルにタイムスタンプは埋めない
    //    （時刻は created_at + frontend のローカル整形）。言語は既定タイトルの言い分けにのみ使う。
    let lang = mojiroku_core::lang::Lang::from_code(load_settings(&app)?.effective_language());
    let rec_title = resolve_recording_title(
        title,
        match lang {
            mojiroku_core::lang::Lang::Ja => "録音",
            mojiroku_core::lang::Lang::En => "Recording",
        },
    );
    let recording = mojiroku_core::Recording {
        id: id.clone(),
        source_type: mojiroku_core::SourceType::Mic,
        title: Some(rec_title),
        duration_ms,
        sample_rate,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    insert_recording_and_maybe_enqueue(&app, &store, &queue, &recording, diarize, record_only, None)
}

/// システム音声収録（画面とシステムオーディオ収録 TCC）の許可状態。会議モードの起動時プリフライト・
/// 更新後の失効検出に使う。true=許可。注意: 許可があっても録れているかは別途 RMS で監視すること。
#[tauri::command]
pub(crate) fn check_system_audio_permission() -> bool {
    system_audio::check_permission()
}

/// 会議録音開始（マイク＝自分 ＋ システム音声＝相手 を同時にキャプチャ）。
/// システム音声（TCC ゲートあり）を先に開始し、続いてマイク。マイク開始に失敗したら
/// システム音声も止めて spool を消し、エラーを返す（片側だけの録音を残さない）。
#[tauri::command]
pub(crate) fn start_meeting_recording(
    app: AppHandle,
    mic: State<'_, mic::MicState>,
    sys: State<'_, system_audio::SystemAudioState>,
    live: State<'_, live_stt::LiveSttState>,
) -> Result<(), String> {
    let spool_dir = resolve_spool_dir(&app)?;
    let session = uuid::Uuid::new_v4();
    system_audio::start(&sys, spool_dir.join(format!("{session}-system.wav")))?;
    if let Err(e) = mic::start(&mic, spool_dir.join(format!("{session}-mic.wav"))) {
        // ロールバック（破棄）: システム音声を止めて spool も消す。
        if let Ok(info) = system_audio::stop(&sys) {
            let _ = std::fs::remove_file(&info.spool_path);
        }
        // e 自体もキーのことがある（例 error.mic.busy）。フロントの translateError は
        // 詳細部を再帰翻訳するので入れ子のまま連結してよい。
        return Err(format!("error.recording.mic_start: {e}"));
    }
    // 両トラック開始後、ライブ文字起こしワーカーを起動する（**best-effort・隔離**: 失敗しても
    // 録音は継続。ライブ表示が出ないだけ）。最後に起動するのでロールバック経路は通らない。
    if let Ok(models_dir) = resolve_models_dir(&app) {
        // whisper への言語ヒントはセッション開始時の設定をスナップショット（best-effort に合わせ
        // 読めなければ既定＝ja）。セッション中の設定変更は次セッションから反映される。
        let stt_lang = load_settings(&app)
            .unwrap_or_default()
            .effective_transcribe_language()
            .map(str::to_string);
        live_stt::start(
            &live,
            app.clone(),
            models_dir,
            mic::live_handle(&mic),
            system_audio::live_handle(&sys),
            stt_lang,
        );
    }
    Ok(())
}

/// 会議録音を破棄停止（文字起こし/保存しない）。会議画面からの離脱時の解放用。
/// 両トラックを止め、spool WAV も削除する。
#[tauri::command]
pub(crate) fn cancel_meeting_recording(
    mic: State<'_, mic::MicState>,
    sys: State<'_, system_audio::SystemAudioState>,
    live: State<'_, live_stt::LiveSttState>,
) -> Result<(), String> {
    live_stt::stop(&live); // 共有バッファを掴むワーカーを先に止めて join
    if let Ok(info) = system_audio::stop(&sys) {
        let _ = std::fs::remove_file(&info.spool_path);
    }
    if let Ok(info) = mic::stop(&mic) {
        let _ = std::fs::remove_file(&info.spool_path);
    }
    Ok(())
}

/// 会議録音停止 → 両トラックを WAV 保存 → 結合ミックス生成 → 文字起こしジョブを投入して**即座に返す**
/// （ADR-0024 非同期フリップ）。音声確定（rename・ミックス）は同期のまま、デュアルトラック STT は
/// ワーカーへ委譲する。両トラックとも無音なら録音行を作る前に明示エラー（whisper の無音ハルシネーション
/// 回避・orphan 防止）。system（相手）は STT＋話者分離、mic（自分）はソース帰属（mic=あなた・ADR-0017）。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn stop_meeting_recording(
    app: AppHandle,
    store: State<'_, SqliteStore>,
    mic: State<'_, mic::MicState>,
    sys: State<'_, system_audio::SystemAudioState>,
    live: State<'_, live_stt::LiveSttState>,
    queue: State<'_, JobQueue>,
    // 「記録を準備」（カレンダー連携）由来の予定タイトル。未指定/空なら既定の「会議」。
    title: Option<String>,
    translations: Option<Vec<mojiroku_core::store::SavedLiveTranslation>>,
) -> Result<StartJobResult, String> {
    let translations = translations.unwrap_or_default();
    mojiroku_core::store::validate_live_translations(&translations)
        .map_err(|e| format!("translation.history_invalid: {e}"))?;

    // 0) ライブワーカーを止めて join する（共有バッファを解放し、本番文字起こしが whisper を
    //    ロードする前に Metal/メモリの競合を避ける。advisor）。
    //    Its live transcript is saved with the recording below (ADR-0048).
    let mut captions = live_stt::stop_and_take(&live);
    // The worker already keeps captions within the stored limits. If they still do not
    // validate, save the meeting without them rather than lose the recording.
    if let Err(e) = mojiroku_core::store::validate_live_captions(&captions) {
        eprintln!("live transcript not saved (recording is saved): {e}");
        captions.clear();
    }
    // 1) 両トラックを停止（spool WAV が finalize される）。片側が落ちても他方で続行する
    //    （録音を丸ごと失わない）。両方とも録れていない場合は下の無音ガードで弾く。
    let sys_info = system_audio::stop(&sys).ok();
    let mic_info = mic::stop(&mic).ok();
    for e in [
        sys_info.as_ref().and_then(|i| i.spool_error.as_ref()),
        mic_info.as_ref().and_then(|i| i.spool_error.as_ref()),
    ]
    .into_iter()
    .flatten()
    {
        eprintln!("会議録音の書き出しで一部エラー（部分保存で続行）: {e}");
    }

    // 相手（system）は peak RMS で非無音判定、自分（mic）は空でなければ採用。
    let sys_has = sys_info
        .as_ref()
        .map(|i| i.samples_written > 0 && i.peak_rms >= system_audio::SILENCE_RMS_THRESHOLD)
        .unwrap_or(false);
    let mic_has = mic_info
        .as_ref()
        .map(|i| i.samples_written > 0)
        .unwrap_or(false);
    if !sys_has && !mic_has {
        // 破棄: spool を残さない。
        if let Some(i) = &sys_info {
            let _ = std::fs::remove_file(&i.spool_path);
        }
        if let Some(i) = &mic_info {
            let _ = std::fs::remove_file(&i.spool_path);
        }
        return Err("error.recording.meeting_silent".into());
    }

    // 2) 採用トラックを正式名へ rename（権威データなので失敗は Err）、非採用は削除。
    //    id は Recording.id と共用。削除は prefix <id> 一致なので -mic / -system も消える。
    let id = uuid::Uuid::new_v4().to_string();
    let rec_dir = resolve_recordings_dir(&app)?;
    std::fs::create_dir_all(&rec_dir).map_err(|e| e.to_string())?;
    let mic_wav = rec_dir.join(format!("{id}-mic.wav"));
    let sys_wav = rec_dir.join(format!("{id}-system.wav"));
    if let Some(i) = &mic_info {
        if mic_has {
            std::fs::rename(&i.spool_path, &mic_wav).map_err(|e| e.to_string())?;
        } else {
            let _ = std::fs::remove_file(&i.spool_path);
        }
    }
    if let Some(i) = &sys_info {
        if sys_has {
            std::fs::rename(&i.spool_path, &sys_wav).map_err(|e| e.to_string())?;
        } else {
            let _ = std::fs::remove_file(&i.spool_path);
        }
    }

    // 録音長は両トラックの最大（会議の長さ）。代表 sample_rate は相手側を優先。
    let mic_rate = mic_info.as_ref().map(|i| i.sample_rate).unwrap_or(16_000);
    let sys_rate = sys_info.as_ref().map(|i| i.sample_rate).unwrap_or(48_000);
    let mic_dur = mic_info
        .as_ref()
        .map(|i| mic::duration_ms(i.samples_written as usize, i.channels, i.sample_rate))
        .unwrap_or(0);
    let sys_dur = sys_info
        .as_ref()
        .filter(|i| i.sample_rate > 0)
        .map(|i| i.samples_written * 1000 / i.sample_rate as u64)
        .unwrap_or(0);
    let duration_ms = mic_dur.max(sys_dur);
    let sample_rate = if sys_has { sys_rate } else { mic_rate };

    // Start offset between the tracks (Issue #65). System capture starts first and blocks
    // until it is ready, then the mic starts, so the mic's first sample lands δ later.
    // Measured from the first audio callback of each track, so it carries about one buffer
    // of delivery latency per side (tens of ms); good enough for turn order, not for
    // sample alignment. Only meaningful when both tracks were kept.
    let mic_offset_ms = match (
        mic_has && sys_has,
        mic_info.as_ref().and_then(|i| i.first_sample_at),
        sys_info.as_ref().and_then(|i| i.first_sample_at),
    ) {
        (true, Some(m), Some(s)) if m >= s => Some((m - s).as_millis() as i64),
        (true, Some(m), Some(s)) => Some(-((s - m).as_millis() as i64)),
        _ => None,
    };
    if let Some(d) = mic_offset_ms {
        eprintln!("meeting tracks: mic started {d} ms after system (stored as mic_offset_ms)");
    }

    // 2.5) 再生用の結合 <id>.wav（mono・48k）を別スレッドで生成する。per-track WAV を
    //    チャンク読み → mono 化 → resample → 加算ミックスのストリーミング処理で、全量を
    //    RAM に乗せない（ADR-0023。従来は数 GB 級の一時バッファだった）。詳細ビューの再生は
    //    この 1 本に統一（File/Mic と同じ <id>.<ext> 規則）。粗いミックス（長尺はδ/ドリフトで
    //    徐々にズレる）だが視聴用途では許容（ADR-0017。文字起こしは per-track の元 WAV で正確）。
    //    Update (Issue #65): the start offset δ is applied as leading silence on the later
    //    track so seeking by transcript time lands on the right audio. Drift is still uncorrected.
    //    best-effort・非致命（失敗しても文字起こしは per-track で成立する）。
    //    **重い ML permit は取らない**（ミックスは resample/加算のみ）: 先行ジョブ実行中でも停止を
    //    塞がず、新しい会議をすぐ始められる（シナリオ⑤）。ただし長尺会議はミックス生成分だけ
    //    停止応答が延びる（STT をブロックしていた旧挙動より遥かに速い。ワーカーは per-track で STT）。
    {
        let combined = rec_dir.join(format!("{id}.wav"));
        let mic_p = mic_has.then(|| mic_wav.clone());
        let sys_p = sys_has.then(|| sys_wav.clone());
        let mix_offset = mic_offset_ms.unwrap_or(0);
        let _ = tauri::async_runtime::spawn_blocking(move || {
            const PLAYBACK_RATE: u32 = 48_000;
            if let Err(e) = crate::audio::mix::write_mixed_wav(
                mic_p.as_deref(),
                sys_p.as_deref(),
                &combined,
                PLAYBACK_RATE,
                mix_offset,
            ) {
                eprintln!("結合 WAV 書き出し失敗（再生用・処理は続行）: {e}");
            }
        })
        .await;
    }

    // 3) Live（会議）録音行（transcript 無し）を作り、文字起こしジョブを積んで即返す。トラック構成
    //    （dual/相手のみ/自分のみ）はワーカーが per-track WAV の存在（上で採用トラックだけ rename・
    //    非採用は削除済み）から判定する。予定タイトルがあれば使い、無ければ既定の「会議」/ "Meeting"。
    //    diarize フラグは会議では無視される（相手は常に話者分離・自分はソース帰属・ADR-0017）ので false。
    let lang = mojiroku_core::lang::Lang::from_code(load_settings(&app)?.effective_language());
    let rec_title = resolve_recording_title(
        title,
        match lang {
            mojiroku_core::lang::Lang::Ja => "会議",
            mojiroku_core::lang::Lang::En => "Meeting",
        },
    );
    let recording = mojiroku_core::Recording {
        id: id.clone(),
        source_type: mojiroku_core::SourceType::Live,
        title: Some(rec_title),
        duration_ms,
        sample_rate,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    // 会議は「音声だけ保存」トグルの対象外（常に文字起こしまで積む）ので record_only=false。
    insert_recording_and_maybe_enqueue_with_live_history(
        &app,
        &store,
        &queue,
        &recording,
        false,
        false,
        mic_offset_ms,
        &translations,
        &captions,
    )
}
