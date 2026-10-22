//! 文字起こし・要約コマンド（File 経路 / ローカル sidecar 要約 / クラウド BYOK 要約）。

use super::*;
use crate::jobs::JobQueue;
use crate::settings;
use tauri::State;

/// 土台の連結確認用コマンド。frontend の invoke('health') → ここ → mojiroku-core。
#[tauri::command]
pub(crate) fn health() -> String {
    mojiroku_core::health()
}

#[derive(serde::Serialize)]
pub(crate) struct TranscriptionModelChoice {
    file: String,
    label: String,
    size: String,
    downloaded: bool,
}

#[derive(serde::Serialize)]
pub(crate) struct TranscriptionModelInfo {
    default_file: String,
    choices: Vec<TranscriptionModelChoice>,
    live_ready: bool,
}

#[tauri::command]
pub(crate) fn transcription_model_info(app: AppHandle) -> Result<TranscriptionModelInfo, String> {
    use mojiroku_core::models::{whisper_model_downloaded, DEFAULT_WHISPER_MODEL, WHISPER_MODELS};
    let models_dir = resolve_models_dir(&app)?;
    Ok(TranscriptionModelInfo {
        default_file: DEFAULT_WHISPER_MODEL.to_string(),
        live_ready: mojiroku_core::models::live_transcription_models_ready(&models_dir),
        choices: WHISPER_MODELS
            .iter()
            .map(|model| TranscriptionModelChoice {
                file: model.file.to_string(),
                label: model.label.to_string(),
                size: if model.size_bytes < 1_000_000_000 {
                    format!("{:.0} MB", model.size_bytes as f64 / 1_000_000.0)
                } else {
                    format!("{:.2} GB", model.size_bytes as f64 / 1_000_000_000.0)
                },
                downloaded: whisper_model_downloaded(model, &models_dir),
            })
            .collect(),
    })
}

/// Download the fixed live-preview models without changing the offline selection.
#[tauri::command]
pub(crate) async fn download_live_transcription_models(app: AppHandle) -> Result<(), String> {
    let models_dir = resolve_models_dir(&app)?;
    // Offline jobs write to the same cache. Share their lock to avoid .part-file races.
    let _permit = acquire_heavy_job(&app, "model://progress").await;
    tauri::async_runtime::spawn_blocking(move || {
        let progress =
            |done, total| emit_progress(&app, "model://progress", "download", done, total);
        mojiroku_core::models::ensure_live_transcription_models(&models_dir, Some(&progress))
            .map_err(core_err)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// What the Settings screen shows: the summary model automatic picks for this Mac, plus
/// the models it can switch to. The explicit choice itself lives in `Settings`
/// (`local_summary_model`), which the UI already holds, so it is not repeated here.
///
/// This is the lifeline against hard-coded strings in the UI. `SettingsView.tsx` used to
/// say `"Qwen2.5-7B Q4_K_M" / "4.4GB"` verbatim. Once the shipped model depends on the
/// Mac's memory (ADR-0030) that text is a lie for many users. Returning the actual
/// selection keeps the display from drifting again.
///
/// `choices` holds **adopted catalog entries only**. Unadopted models are not offered:
/// core would fall back to auto anyway, so the row would look selectable but do nothing.
#[derive(serde::Serialize)]
pub(crate) struct SummaryModelInfo {
    /// What automatic picks (model already on disk first, then tier). This runs when the
    /// explicit choice is empty, and it is what the UI must show the moment the user
    /// switches back to automatic. Sent as a full entry rather than a file name because
    /// auto may resolve to a model on disk that is not in `choices` (a hand-placed,
    /// unadopted file).
    auto: SummaryModelChoice,
    /// Switch targets: adopted models only, in ascending tier order.
    choices: Vec<SummaryModelChoice>,
}

/// One summary model as the Settings screen sees it.
#[derive(serde::Serialize)]
pub(crate) struct SummaryModelChoice {
    /// e.g. `Qwen3.5-9B-Q4_K_M.gguf`.
    file: String,
    /// e.g. `Qwen3.5-9B Q4_K_M` (extension dropped, quantization split off for display).
    label: String,
    /// Display size, e.g. `5.7GB`.
    size: String,
    /// Already on this Mac. If not, the next summary downloads it.
    downloaded: bool,
    tier: mojiroku_core::models::SummaryTier,
    /// Above this Mac's tier. **Still selectable**: Issue #30 asks that the user can go
    /// up "after being shown the re-download size". The UI attaches a warning.
    exceeds_tier: bool,
}

/// `Qwen3.5-9B-Q4_K_M.gguf` → `Qwen3.5-9B Q4_K_M` (only the last `-Q…` becomes a space).
fn summary_model_label(file: &str) -> String {
    let stem = file.trim_end_matches(".gguf");
    match stem.rfind("-Q") {
        Some(i) => format!("{} {}", &stem[..i], &stem[i + 1..]),
        None => stem.to_string(),
    }
}

/// Display size in decimal GB (`5.7GB`), matching the download progress display.
fn summary_model_size(bytes: u64) -> String {
    format!("{:.1}GB", bytes as f64 / 1_000_000_000.0)
}

#[tauri::command]
pub(crate) fn summary_model_info(app: AppHandle) -> Result<SummaryModelInfo, String> {
    use mojiroku_core::models::{
        select_summary_model, tier_for_memory, SummaryModel, SUMMARY_MODELS,
    };
    let models_dir = resolve_models_dir(&app)?;
    let mem = mojiroku_core::hardware::total_memory_bytes();
    let tier = tier_for_memory(mem);
    let entry = |c: &SummaryModel| SummaryModelChoice {
        file: c.file.to_string(),
        label: summary_model_label(c.file),
        size: summary_model_size(c.size_bytes),
        downloaded: models_dir.join(c.file).exists(),
        tier: c.tier,
        exceeds_tier: c.tier > tier,
    };
    Ok(SummaryModelInfo {
        auto: entry(select_summary_model(mem, &models_dir)),
        choices: SUMMARY_MODELS
            .iter()
            .filter(|c| c.adopted)
            .map(&entry)
            .collect(),
    })
}

/// 音声ファイル → 文字起こしジョブを投入（ADR-0024 非同期フリップ）。
/// 原本を `recordings/<id>.<ext>` へ確定コピー（**ワーカーの STT 入力になる正本**）してから
/// 録音行（transcript 無し）を作りジョブを積み、**即座に返す**。STT/話者分離はワーカーが 1 本ずつ回す。
/// 呼び出し側は `recording_id` で DetailView へ遷移し、`job://update` で進捗を追う。
/// `record_only=true`（増分5「音声だけ保存」）は録音行だけ作ってジョブは積まず、後で処理する。
#[tauri::command]
pub(crate) async fn transcribe_file(
    app: AppHandle,
    store: State<'_, SqliteStore>,
    queue: State<'_, JobQueue>,
    path: String,
    diarize: bool,
    record_only: bool,
) -> Result<StartJobResult, String> {
    let id = uuid::Uuid::new_v4().to_string();

    // 原本を recordings/<id>.<ext> へ複製する。フリップ後はワーカーがこのコピーを STT で読むため、
    // **成功は必須**（旧 best-effort から致命へ格上げ）。失敗したら録音行もジョブも作らず即エラー
    // ＝ orphan（音声の無い doomed 録音）を残さない。再エンコードせず拡張子を保持
    // （2h の m4a を WAV 化すると肥大するため）。
    copy_recording_original(&app, &id, &path).await?;

    // duration は enqueue 時 0（暫定）。ジョブ完了の replace_transcript で最終 segment 末尾へ確定。
    let title = std::path::Path::new(&path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());
    let recording = mojiroku_core::Recording {
        id: id.clone(),
        source_type: mojiroku_core::SourceType::File,
        title,
        duration_ms: 0,
        sample_rate: mojiroku_core::audio::WHISPER_SAMPLE_RATE,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    insert_recording_and_maybe_enqueue(&app, &store, &queue, &recording, diarize, record_only, None)
}

/// 文字起こし結果 → 要約/議事録（Phase 1b・ローカル既定）。
/// ggml 衝突回避のため、ローカル要約は別バイナリ `mojiroku-llm`（sidecar）で実行する（ADR-0007）。
/// 本体は LLM モデルを確保し、プロンプトを temp に書いて sidecar に渡し、stdout を受け取る。
#[tauri::command]
pub(crate) async fn summarize(
    app: AppHandle,
    store: State<'_, SqliteStore>,
    transcript: mojiroku_core::Transcript,
    recording_id: String,
    template_id: String,
) -> Result<mojiroku_core::Summary, String> {
    // 要約エンジン設定を読む。cloud（BYOK）ならクラウド経路へ分岐する。
    // 以降のローカル sidecar 経路は従来どおり（純加算ブランチ＝ローカル経路は不変）。
    let cfg = load_settings(&app)?;
    // 要約の出力言語（テンプレ instruction・プロンプトのマーカー・sidecar/BYOK の
    // システムプロンプトに反映）。未設定の旧 settings.json は ja。
    let lang = mojiroku_core::lang::Lang::from_code(cfg.effective_language());
    if cfg.engine == "cloud" {
        let template = mojiroku_core::summarize::template_by_id(&template_id, lang);
        let summary = summarize_cloud(&app, transcript, template, &cfg).await?;
        store
            .save_summary(&recording_id, &summary)
            .map_err(|e| e.to_string())?;
        return Ok(summary);
    }

    let models_dir = resolve_models_dir(&app)?;

    // 重い ML ジョブを直列化: ローカル要約 sidecar は 4.4GB モデルを積むため、
    // 文字起こし/話者分離と同時に走らせない（メモリ枯渇によるクラッシュ/フリーズ予防）。
    // permit は sidecar 完了まで保持する。
    let _heavy_permit = acquire_heavy_job(&app, "summarize://progress").await;

    // 1) LLM モデル確保（必要なら DL, blocking）+ プロンプト構築
    // The explicit switch from Settings; empty = automatic. Owned because the closure moves.
    let requested = cfg.requested_local_summary_model().map(str::to_owned);
    let app_dl = app.clone();
    let template_id2 = template_id.clone();
    let (model_path, prompt) = tauri::async_runtime::spawn_blocking(
        move || -> Result<(std::path::PathBuf, String), String> {
            let dl_cb = |done: u64, total: Option<u64>| {
                emit_progress(&app_dl, "summarize://progress", "download_llm", done, total)
            };
            // Pick the summary model for this Mac (ADR-0030).
            //
            // Precedence: explicit choice in Settings → model already on disk → tier.
            // The cached model wins over the tier: `select_summary_model` checks the
            // cache first, so a user who already has 7B never gets a multi-GB re-download.
            // The tier only matters on a fresh install. Switching is an explicit action in
            // Settings, and only then does the setting beat the cache
            // (`select_summary_model_with`).
            let model = mojiroku_core::models::select_summary_model_with(
                requested.as_deref(),
                mojiroku_core::hardware::total_memory_bytes(),
                &models_dir,
            );
            let model_path = mojiroku_core::models::ensure_model(
                model.file,
                &mojiroku_core::models::summary_model_url(model.file),
                &models_dir,
                Some(&dl_cb),
            )
            .map_err(core_err)?;
            let template = mojiroku_core::summarize::template_by_id(&template_id2, lang);
            let prompt = mojiroku_core::summarize::build_prompt(&transcript, &template, lang);
            Ok((model_path, prompt))
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    emit_progress(&app, "summarize://progress", "summarize", 0, None);
    let stdout = run_local_llm(&app, &model_path, &prompt, lang, None).await?;
    let content = mojiroku_core::summarize::tidy_local_output(&stdout);
    let summary = mojiroku_core::Summary {
        template_id,
        content,
        action_items: Vec::new(),
        stale: false,
    };
    // 同一 recording に要約を追加保存（save_summary は同期メソッドで .await を跨がない）。
    store
        .save_summary(&recording_id, &summary)
        .map_err(|e| e.to_string())?;
    Ok(summary)
}

/// タイトル生成で sidecar に許す最大トークン数。プロンプトの調整（Issue #4）はこの値で測った。
pub(crate) const TITLE_MAX_TOKENS: u32 = 48;

/// 手元にある要約モデルのパス（設定の選択を尊重し、ダウンロードはしない）。無ければ `None`。
pub(crate) fn cached_summary_model_path(
    app: &AppHandle,
    models_dir: &std::path::Path,
) -> Result<Option<std::path::PathBuf>, String> {
    let cfg = load_settings(app)?;
    Ok(mojiroku_core::models::cached_summary_model(
        cfg.requested_local_summary_model(),
        mojiroku_core::hardware::total_memory_bytes(),
        models_dir,
    )
    .map(|m| models_dir.join(m.file)))
}

/// 詳細画面の「タイトルを生成」（Issue #4）。利用者の明示の操作なので、要約と同じエンジン設定に
/// 従う（クラウド設定ならクラウドへ送る）。ローカルは手元の要約モデルを使い、無ければダウンロードを
/// 始めずにエラーを返す。成功したら保存して新しいタイトルを返す。
///
/// 自動生成（`jobs.rs` の title ジョブ）とは違い、既定名以外のタイトルも置き換える。
#[tauri::command]
pub(crate) async fn generate_title(
    app: AppHandle,
    store: State<'_, SqliteStore>,
    recording_id: String,
) -> Result<String, String> {
    let cfg = load_settings(&app)?;
    let lang = mojiroku_core::lang::Lang::from_code(cfg.effective_language());
    let transcript = store
        .get_recording_detail(&recording_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "error.recording.not_found".to_string())?
        .transcript;
    if transcript.segments.is_empty() {
        return Err("error.job.no_transcript".to_string());
    }

    let raw = if cfg.engine == "cloud" {
        let template = mojiroku_core::summarize::title_template(lang);
        summarize_cloud(&app, transcript, template, &cfg).await?.content
    } else {
        let models_dir = resolve_models_dir(&app)?;
        let model_path = cached_summary_model_path(&app, &models_dir)?
            .ok_or_else(|| "error.title.model_missing".to_string())?;
        let _heavy_permit = acquire_heavy_job(&app, "title://progress").await;
        let prompt = mojiroku_core::summarize::build_title_prompt(&transcript, lang);
        run_local_llm(&app, &model_path, &prompt, lang, Some(TITLE_MAX_TOKENS)).await?
    };

    let title = mojiroku_core::summarize::sanitize_title(&raw)
        .ok_or_else(|| "error.title.not_generated".to_string())?;
    store
        .rename_recording(&recording_id, Some(&title))
        .map_err(|e| e.to_string())?;
    Ok(title)
}

/// ローカル要約 sidecar（`mojiroku-llm`, ADR-0007）を 1 回実行して標準出力を返す。要約とタイトル生成で共通。
/// 呼び出し側が重い処理の許可（HEAVY_ML_JOB）を持っていること。`max_tokens` を省くと sidecar の既定。
pub(crate) async fn run_local_llm(
    app: &AppHandle,
    model_path: &std::path::Path,
    prompt: &str,
    lang: mojiroku_core::lang::Lang,
    max_tokens: Option<u32>,
) -> Result<String, String> {
    use tauri_plugin_shell::ShellExt;

    // プロンプトは temp ファイルで渡す（巨大な文字起こしを引数で渡さない）。呼び出しごとに別名。
    let prompt_file = std::env::temp_dir().join(format!("mojiroku-prompt-{}.txt", uuid::Uuid::new_v4()));
    std::fs::write(&prompt_file, prompt).map_err(|e| e.to_string())?;

    let mut args = vec![
        model_path.to_string_lossy().to_string(),
        prompt_file.to_string_lossy().to_string(),
    ];
    if let Some(n) = max_tokens {
        args.push(n.to_string());
    }
    args.push("--lang".to_string());
    args.push(lang.code().to_string());
    // 思考モデル（Qwen3 系）には `--no-think` が要る。渡さないと**英語の
    // `<think>` ブロックがそのまま stdout に出て**、利用者には議事録の代わりに
    // 思考トレースが見える（2026-08-30 に実測）。
    //
    // 無条件に渡してはいけない。このフラグはプロンプトに `<think></think>` を
    // 足すので、思考しないモデルでは出力が変わる（Qwen2.5 で文言が変化した）。
    // 渡すかどうかはモデルの属性（`SummaryModel::thinking`）が決める。
    if mojiroku_core::models::needs_no_think(&model_path.file_name().unwrap_or_default().to_string_lossy()) {
        args.push("--no-think".to_string());
    }

    let result = app
        .shell()
        .sidecar("mojiroku-llm")
        .map_err(|e| e.to_string())?
        .args(args)
        .output()
        .await
        .map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&prompt_file);
    let output = result?;
    if !output.status.success() {
        return Err(format!(
            "error.summarize.sidecar_failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// クラウド（BYOK）要約。鍵はキーチェーンから Rust 内で直接取得し、webview へ往復させない。
/// ⚠️ BYOK 利用時はデータが端末外（各プロバイダ）へ送信される（プライバシーのトレードオフ）。
async fn summarize_cloud(
    app: &AppHandle,
    transcript: mojiroku_core::Transcript,
    template: mojiroku_core::SummaryTemplate,
    cfg: &settings::Settings,
) -> Result<mojiroku_core::Summary, String> {
    use mojiroku_core::summarize::{AnthropicSummarizer, OpenAiSummarizer, SummarizeProvider};

    let model = cfg.effective_model(); // 空文字を API に送らない（既定へ解決済み）
    let provider = cfg.provider.clone();
    let key_name = secrets::byok_key_name(&provider);
    // 要約の出力言語（テンプレ instruction・システムプロンプトに反映）。
    let lang = mojiroku_core::lang::Lang::from_code(cfg.effective_language());

    emit_progress(app, "summarize://progress", "summarize", 0, None);

    // キーチェーン取得（許可ダイアログでブロックし得る）と ureq はどちらも blocking。
    // tokio ワーカーをブロックしないよう spawn_blocking で一括して回す。
    tauri::async_runtime::spawn_blocking(move || -> Result<mojiroku_core::Summary, String> {
        let api_key = get_secret_or_error(&key_name, "error.summarize.api_key_missing")?;
        let result = match provider.as_str() {
            "openai" => OpenAiSummarizer {
                api_key,
                model,
                lang,
            }
            .summarize(&transcript, &template),
            _ => AnthropicSummarizer {
                api_key,
                model,
                lang,
            }
            .summarize(&transcript, &template),
        };
        result.map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
