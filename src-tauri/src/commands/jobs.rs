//! バックグラウンドジョブ投入コマンド（ADR-0024）。
//!
//! 既存録音を対象に「文字起こし」「後付け話者分離」を永続キューへ積み、即座に返す。
//! 実処理はワーカー（`crate::jobs`）が `HEAVY_ML_JOB` を 1 本ずつ取って回す。
//! **設定（言語・話者分離指定）は enqueue 時にスナップショット**して `JobParams` に固めるので、
//! キュー待機中に設定を変えても各ジョブは投入時点の指定で走る（ワーカーは live 設定を読まない）。

use super::*;
use crate::jobs::JobQueue;
use mojiroku_core::store::JobParams;
use tauri::State;

/// 現在のコンテンツ言語・STT 言語ヒントを `JobParams` へスナップショットする。
/// `super::insert_and_enqueue_transcribe`（録音停止・ファイル取り込みのフリップ）からも使う。
pub(crate) fn snapshot_params(app: &AppHandle, diarize: bool) -> Result<JobParams, String> {
    let cfg = load_settings(app)?;
    Ok(params_from_settings(&cfg, diarize))
}

fn params_from_settings(cfg: &crate::settings::Settings, diarize: bool) -> JobParams {
    JobParams {
        diarize,
        stt_lang: cfg.effective_transcribe_language().map(str::to_string),
        transcription_model: mojiroku_core::models::select_whisper_model(Some(
            &cfg.transcription_model,
        ))
        .file
        .to_string(),
        lang: cfg.effective_language().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_language_mode_survives_settings_changes_and_serialization() {
        let mut cfg = crate::settings::Settings {
            transcribe_language: mojiroku_core::stt::MIXED_LANGUAGE_MODE.into(),
            ..Default::default()
        };
        let queued = params_from_settings(&cfg, false);
        cfg.transcribe_language = "auto".into();
        let restored: JobParams =
            serde_json::from_str(&serde_json::to_string(&queued).unwrap()).unwrap();
        assert_eq!(restored.stt_lang.as_deref(), Some("mixed"));
        assert_eq!(params_from_settings(&cfg, false).stt_lang, None);
    }

    #[test]
    fn queued_model_is_independent_of_later_settings_changes() {
        use mojiroku_core::models::{DEFAULT_WHISPER_MODEL, FULL_WHISPER_MODEL};
        let mut cfg = crate::settings::Settings {
            transcription_model: FULL_WHISPER_MODEL.to_string(),
            ..Default::default()
        };
        let queued = params_from_settings(&cfg, true);
        cfg.transcription_model = DEFAULT_WHISPER_MODEL.to_string();
        assert_eq!(queued.transcription_model, FULL_WHISPER_MODEL);
        assert!(queued.diarize);
        assert_eq!(
            params_from_settings(&cfg, false).transcription_model,
            DEFAULT_WHISPER_MODEL
        );
        cfg.transcription_model = "unknown-model.bin".to_string();
        assert_eq!(
            params_from_settings(&cfg, false).transcription_model,
            DEFAULT_WHISPER_MODEL
        );
    }
}

/// 既存録音を（再）文字起こしするジョブを投入する。`diarize=true` なら STT に話者分離も含める。
/// 録音のみ保存（音声だけ先に確定）した行を後から文字起こしする経路でもある（ADR-0024）。
#[tauri::command]
pub(crate) fn transcribe_recording(
    app: AppHandle,
    store: State<'_, SqliteStore>,
    queue: State<'_, JobQueue>,
    recording_id: String,
    diarize: bool,
) -> Result<StartJobResult, String> {
    let params = snapshot_params(&app, diarize)?;
    let job_id = uuid::Uuid::new_v4().to_string();
    store
        .enqueue_job(&job_id, &recording_id, "transcribe", &params)
        .map_err(|e| e.to_string())?;
    queue.wake();
    Ok(StartJobResult {
        recording_id,
        job_id: Some(job_id),
    })
}

/// 既存録音に**後から話者分離**を掛けるジョブを投入する（ベスト努力で表示名を引き継ぐ・ADR-0024）。
/// 文字起こし済みが前提（本文が無ければワーカーがエラーにする）。話者が既に付いていても
/// やり直せる（Issue #102）。会議（Live）は system（相手）トラックだけを分離し直し、自分（`self`）の
/// セグメントには触れない。
#[tauri::command]
pub(crate) fn diarize_recording(
    app: AppHandle,
    store: State<'_, SqliteStore>,
    queue: State<'_, JobQueue>,
    recording_id: String,
) -> Result<StartJobResult, String> {
    // diarize ジョブに話者分離フラグは不要だが、params の形は共通なので false を入れる。
    let params = snapshot_params(&app, false)?;
    let job_id = uuid::Uuid::new_v4().to_string();
    store
        .enqueue_job(&job_id, &recording_id, "diarize", &params)
        .map_err(|e| e.to_string())?;
    queue.wake();
    Ok(StartJobResult {
        recording_id,
        job_id: Some(job_id),
    })
}

/// 進行中・要注意なジョブ一覧（pending/running/failed、更新の新しい順）。
/// UI のキュー表示・履歴行の処理中/失敗バッジ用。完了（done/canceled）は含めない。
/// タイトル生成（Issue #4）は裏方なので出さない。
#[tauri::command]
pub(crate) fn list_jobs(
    store: State<'_, SqliteStore>,
) -> Result<Vec<mojiroku_core::store::Job>, String> {
    let jobs = store
        .list_jobs(&["pending", "running", "failed"])
        .map_err(|e| e.to_string())?;
    Ok(jobs
        .into_iter()
        .filter(|j| j.kind != mojiroku_core::store::TITLE_JOB_KIND)
        .collect())
}

/// ジョブをキャンセルする。pending はその場で canceled、running は中断を求め、ワーカーが
/// 止まった時点で canceled として `job://update` を出す（Issue #114）。受け付けたら true。
#[tauri::command]
pub(crate) fn cancel_job(
    store: State<'_, SqliteStore>,
    queue: State<'_, crate::jobs::JobQueue>,
    job_id: String,
) -> Result<bool, String> {
    if store.cancel_job(&job_id).map_err(|e| e.to_string())? {
        return Ok(true);
    }
    Ok(queue.request_cancel(&job_id))
}
