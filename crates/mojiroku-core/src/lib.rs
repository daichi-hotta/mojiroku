//! mojiroku-core — ML パイプライン（STT / 話者分離 / 要約）。
//!
//! 本クレートは UI / Tauri に依存しない。データモデル（`schemas`）と各エンジン
//! （whisper.cpp STT / sherpa-onnx 話者分離 / llama.cpp 要約）を高レベル関数
//! （`transcribe_file` / `transcribe_and_diarize_file` 等）に結線して提供する。
//! 設計は `docs/03_design/spec.md` / `docs/05_decisions/` を参照。

pub mod error;
pub mod schemas;

pub mod audio;
pub mod calendar;
pub mod cancel;
pub mod diarization;
pub mod export;
pub mod ffi_guard;
pub mod hardware;
pub mod lang;
pub mod merge;
pub mod models;
pub mod store;
pub mod stt;
pub mod summarize;
pub mod vad;

use std::path::{Path, PathBuf};

pub use error::{CoreError, Result};
pub use schemas::*;

/// Offline transcription choices, captured before a job starts.
#[derive(Clone, Copy)]
pub struct TranscriptionOptions<'a> {
    pub language: Option<&'a str>,
    pub model: &'static models::WhisperModel,
    pub decoding: stt::DecodingStrategy,
}

impl Default for TranscriptionOptions<'_> {
    fn default() -> Self {
        Self {
            language: None,
            model: models::select_whisper_model(None),
            decoding: stt::FILE_DECODING,
        }
    }
}

/// クロスクレート経路の実証用。
/// frontend の `invoke('health')` → `#[tauri::command] health` → ここ、で土台の連結を確認する。
pub fn health() -> String {
    format!("mojiroku-core v{} ok", env!("CARGO_PKG_VERSION"))
}

/// 進捗ステージ通知: `(stage, done_bytes, total_bytes_opt)`。
/// stage は "download" / "decode" / "transcribe"。
pub type StageProgressFn<'a> = dyn Fn(&str, u64, Option<u64>) + 'a;

/// `models::ensure_*` が要求する `Fn(u64, Option<u64>)` 形へ `on_progress` を適合させる薄いラッパ。
/// DL のバイト進捗を "download" ステージとして転送する（各高レベル関数で重複していたクロージャを集約）。
fn download_progress<'a>(
    on_progress: Option<&'a StageProgressFn<'a>>,
) -> impl Fn(u64, Option<u64>) + 'a {
    move |done, total| {
        if let Some(cb) = on_progress {
            cb("download", done, total);
        }
    }
}

/// `(stage, 0, None)` 形のステージ開始通知を送る薄いヘルパ（各高レベル関数で重複していた
/// `if let Some(cb) = on_progress { cb("...", 0, None); }` を集約）。
/// 話者分離のターンを書き出す（Issue #65 の調査用）。`MOJIROKU_DEBUG_TURNS=<path>` のときだけ、
/// 発話の話者の付き方をデバッガなしで確かめられるよう JSON で保存する。失敗しても処理は続ける。
fn debug_dump_turns(turns: &[diarization::SpeakerTurn]) {
    let Some(path) = std::env::var_os("MOJIROKU_DEBUG_TURNS") else {
        return;
    };
    let rows: Vec<_> = turns
        .iter()
        .map(|t| serde_json::json!({"start_ms": t.start_ms, "end_ms": t.end_ms, "speaker_id": t.speaker_id}))
        .collect();
    if let Err(e) = std::fs::write(&path, serde_json::to_vec_pretty(&rows).unwrap_or_default()) {
        eprintln!("MOJIROKU_DEBUG_TURNS: could not write {}: {e}", std::path::Path::new(&path).display());
    }
}

/// 段階の境目。中断が求められていればここで止め（Issue #114）、そうでなければ段階を通知する。
fn report_stage(on_progress: Option<&StageProgressFn<'_>>, stage: &str) -> Result<()> {
    cancel::check()?;
    if let Some(cb) = on_progress {
        cb(stage, 0, None);
    }
    Ok(())
}

/// VAD モデルを確保（best-effort・DL 進捗は流さない）→ whisper をロード → PCM を文字起こしする
/// 共通処理。transcribe_file / transcribe_and_diarize_file で逐語重複していた STT サブブロックを集約。
fn transcribe_pcm(
    model_path: &Path,
    models_dir: &Path,
    pcm: &[f32],
    language: Option<&str>,
    decoding: stt::DecodingStrategy,
    on_pct: Option<&dyn Fn(i32)>,
) -> Result<schemas::Transcript> {
    // VAD モデル（無音ハルシネーション対策）。取得失敗時は VAD 無しで続行（best-effort）。
    let vad_path = models::ensure_model(
        models::DEFAULT_VAD_MODEL,
        &models::vad_model_url(models::DEFAULT_VAD_MODEL),
        models_dir,
        None,
    )
    .ok();
    let engine = stt::WhisperStt::load(model_path, vad_path)?;
    engine.transcribe_with_decoding(pcm, language, decoding, on_pct)
}

/// 話者分離モデル（segmentation + embedding）を同順で確保する共通処理。
/// transcribe_and_diarize_file / diarize_file で逐語重複していた 2 つの ensure を集約。
/// **呼び出しは各関数の既存位置のまま**にすること（DL 進捗の発火順を変えないため。
/// ensure と diarize を 1 関数に畳まないこと）。
fn ensure_diar_models(
    models_dir: &Path,
    on_progress: Option<&models::ProgressFn<'_>>,
) -> Result<(PathBuf, PathBuf)> {
    let seg = models::ensure_diar_seg_model(models_dir, on_progress)?;
    let emb = models::ensure_model(
        models::DEFAULT_DIAR_EMB_MODEL,
        models::diar_emb_url(),
        models_dir,
        on_progress,
    )?;
    Ok((seg, emb))
}

/// 高レベル: 音声ファイル → 文字起こし。
/// モデル確保（必要なら DL）→ デコード(16k mono) → STT の 1 本道（Phase 1a）。
pub fn transcribe_file(
    audio_path: &Path,
    models_dir: &Path,
    language: Option<&str>,
    on_progress: Option<&StageProgressFn<'_>>,
) -> Result<schemas::Transcript> {
    transcribe_file_with_decoding(
        audio_path,
        models_dir,
        language,
        stt::FILE_DECODING,
        on_progress,
    )
}

/// The product file pipeline with an explicit decoder for reproducible A/B evaluation.
pub fn transcribe_file_with_decoding(
    audio_path: &Path,
    models_dir: &Path,
    language: Option<&str>,
    decoding: stt::DecodingStrategy,
    on_progress: Option<&StageProgressFn<'_>>,
) -> Result<schemas::Transcript> {
    transcribe_file_with_options(
        audio_path,
        models_dir,
        TranscriptionOptions {
            language,
            decoding,
            ..Default::default()
        },
        on_progress,
    )
}

/// The product file pipeline with explicit offline model and decoding choices.
pub fn transcribe_file_with_options(
    audio_path: &Path,
    models_dir: &Path,
    options: TranscriptionOptions<'_>,
    on_progress: Option<&StageProgressFn<'_>>,
) -> Result<schemas::Transcript> {
    transcribe_file_impl(audio_path, models_dir, options, on_progress, true)
}

/// `transcribe_file` の実体。`emit_pct` で whisper 0-100% の転送を制御する。
/// `emit_pct=false` は会議 dual-track の **mic 側サブ STT** 用（[`transcribe_meeting_dual_track`]）。
/// 会議は full() が複数回走り %が 0→100 を繰り返す（途中でリセット＝「残り約N分」が巻き戻って
/// 壊れて見える）ため、%は**単一ファイル文字起こしだけ**に出す（会議/話者分離は経過時間で示す。
/// ADR-0024 / advisor）。
fn transcribe_file_impl(
    audio_path: &Path,
    models_dir: &Path,
    options: TranscriptionOptions<'_>,
    on_progress: Option<&StageProgressFn<'_>>,
    emit_pct: bool,
) -> Result<schemas::Transcript> {
    // 1) モデル確保
    let dl_cb = download_progress(on_progress);
    let model_path = models::ensure_model(
        options.model.file,
        &models::whisper_model_url(options.model.file),
        models_dir,
        Some(&dl_cb),
    )?;

    // 2) デコード（16kHz mono f32）
    report_stage(on_progress, "decode")?;
    let pcm = audio::decode_to_pcm16k_mono(audio_path)?;

    // 3) STT。whisper 0-100% を transcribe ステージの done/total(=Some(100)) として流す
    //    （フロントはこの total 有無で ETA を出すか決める）。
    report_stage(on_progress, "transcribe")?;
    let pct_adapter;
    let on_pct: Option<&dyn Fn(i32)> = match (emit_pct, on_progress) {
        (true, Some(cb)) => {
            pct_adapter = move |pct: i32| cb("transcribe", pct.max(0) as u64, Some(100));
            Some(&pct_adapter)
        }
        _ => None,
    };
    transcribe_pcm(
        &model_path,
        models_dir,
        &pcm,
        options.language,
        options.decoding,
        on_pct,
    )
}

/// 高レベル: 音声ファイル → 文字起こし＋話者分離（話者付き Transcript）。
/// 1 プロセスで whisper(STT) → sherpa(diarization) を順に走らせ、結果をマージする。
/// （ggml × onnxruntime の同居は安全。ADR-0009）。重い 2 段なので呼び出しは opt-in 想定。
///
/// トポロジ（ADR-0009）: STT は VAD 経由（無音ハルシネーション対策、時刻は原時刻へ再マップ）、
/// diarization は**同じ原音声 PCM**に対し別途実行。両者を時間重なりでマージする。
///
/// `language` は whisper への言語ヒント（None=自動判定）、`lang` は生成する話者ラベル等の
/// コンテンツ言語。別物なので注意（例: 文字起こし auto ＋ ラベル ja があり得る）。
pub fn transcribe_and_diarize_file(
    audio_path: &Path,
    models_dir: &Path,
    language: Option<&str>,
    lang: lang::Lang,
    on_progress: Option<&StageProgressFn<'_>>,
) -> Result<(
    schemas::Transcript,
    Vec<schemas::Speaker>,
    Vec<diarization::SpeakerEmbedding>,
)> {
    transcribe_and_diarize_file_with_options(
        audio_path,
        models_dir,
        TranscriptionOptions {
            language,
            ..Default::default()
        },
        lang,
        on_progress,
    )
}

/// Offline model selection also applies when speaker separation is requested.
pub fn transcribe_and_diarize_file_with_options(
    audio_path: &Path,
    models_dir: &Path,
    options: TranscriptionOptions<'_>,
    lang: lang::Lang,
    on_progress: Option<&StageProgressFn<'_>>,
) -> Result<(
    schemas::Transcript,
    Vec<schemas::Speaker>,
    Vec<diarization::SpeakerEmbedding>,
)> {
    let dl_cb = download_progress(on_progress);

    // 1) whisper モデル確保 + 原音声を 16k mono へデコード（1 回だけ。STT/diar で共有）
    let model_path = models::ensure_model(
        options.model.file,
        &models::whisper_model_url(options.model.file),
        models_dir,
        Some(&dl_cb),
    )?;
    report_stage(on_progress, "decode")?;
    let pcm = audio::decode_to_pcm16k_mono(audio_path)?;

    // 2) STT（VAD 経由）。話者分離込みの経路では %を出さない（後段 diarization/merge が続き、
    //    %が transcribe だけ 0→100 して見えるのは誤解を招く。経過時間で示す・on_pct=None）。
    report_stage(on_progress, "transcribe")?;
    let mut transcript = transcribe_pcm(
        &model_path,
        models_dir,
        &pcm,
        options.language,
        options.decoding,
        None,
    )?;

    // 3) diarization（同じ原音声 PCM。VAD を通さない）
    report_stage(on_progress, "diarization")?;
    let (seg, emb) = ensure_diar_models(models_dir, Some(&dl_cb))?;
    let diarizer = diarization::SherpaDiarizer::new(seg, emb, diarization::DEFAULT_THRESHOLD, lang);
    use diarization::Diarizer;
    let diar = diarizer.diarize(&pcm, 16_000)?;
    // sherpa-onnx は途中で止められないので、終わった直後に確かめる（Issue #114 レビュー）。
    cancel::check()?;

    debug_dump_turns(&diar.turns);

    // 4) マージ（話者 turn → Segment.speaker_id）
    report_stage(on_progress, "merge")?;
    merge::assign_speakers(&mut transcript, &diar);
    Ok((transcript, diar.speakers, diar.embeddings))
}

/// 高レベル: 会議モードのデュアルトラック文字起こし。マイク（自分）とシステム音声（相手）の
/// 2 ファイルを別々に STT し、ソースで合成する（ADR-0017）。
/// - system: STT＋話者分離（複数の相手話者を分離）。
/// - mic: STT のみ（単一話者＝あなた）。
///
/// ソース帰属が構造上保証されるためクロックドリフトに免疫（近接する異トラック発話の並び順が
/// わずかに乱れる cosmetic な影響のみ）。重い 2 STT＋1 diarization。空トラック（無音）は
/// そのまま空 Transcript として合成される。
///
/// `mic_offset_ms`: how much later the mic track started than the system track (Issue #65),
/// as stored on the recording; 0 when unknown. Applied in [`merge::merge_tracks`].
pub fn transcribe_meeting_dual_track(
    mic_path: &Path,
    system_path: &Path,
    models_dir: &Path,
    language: Option<&str>,
    lang: lang::Lang,
    mic_offset_ms: i64,
    on_progress: Option<&StageProgressFn<'_>>,
) -> Result<(
    schemas::Transcript,
    Vec<schemas::Speaker>,
    Vec<diarization::SpeakerEmbedding>,
)> {
    transcribe_meeting_dual_track_with_options(
        mic_path,
        system_path,
        models_dir,
        TranscriptionOptions {
            language,
            ..Default::default()
        },
        lang,
        mic_offset_ms,
        on_progress,
    )
}

/// Use the same captured model for both tracks of a recorded meeting.
pub fn transcribe_meeting_dual_track_with_options(
    mic_path: &Path,
    system_path: &Path,
    models_dir: &Path,
    options: TranscriptionOptions<'_>,
    lang: lang::Lang,
    mic_offset_ms: i64,
    on_progress: Option<&StageProgressFn<'_>>,
) -> Result<(
    schemas::Transcript,
    Vec<schemas::Speaker>,
    Vec<diarization::SpeakerEmbedding>,
)> {
    // 相手（システム音声）: STT＋話者分離（声紋も取得）。
    let (system, system_speakers, system_embeddings) = transcribe_and_diarize_file_with_options(
        system_path,
        models_dir,
        options,
        lang,
        on_progress,
    )?;
    // 自分（マイク）: STT のみ。
    // mic 側は %を抑止（会議は system STT/diarization/mic STT/merge と多段。mic の 0→100 だけ
    //    出すと全体進捗と誤読される。会議は経過時間で示す・emit_pct=false）。
    let mic = transcribe_file_impl(mic_path, models_dir, options, on_progress, false)?;
    cancel::check()?;
    // ソース合成（マイク=self、システム=diarization 話者を保持、時系列マージ）。
    // system 話者 id は merge_tracks で不変＝声紋（system_embeddings）の id とも整合する。
    let (transcript, speakers) =
        merge::merge_tracks(mic, system, system_speakers, lang, mic_offset_ms);
    Ok((transcript, speakers, system_embeddings))
}

/// 高レベル: 音声ファイル → 話者分離（誰がいつ話したか）。
/// モデル確保（pyannote seg-3.0 tar.bz2 展開 + TitaNet）→ デコード(16k mono) → diarization。
///
/// **トポロジ注意**（ADR-0009）: diarization は **原音声**（VAD で無音除去する前）に対して
/// 走らせる。VAD→STT とは別ブランチで、結果はマージ段で時間重なり結合する。ここでは
/// `decode_to_pcm16k_mono` の生 PCM をそのまま渡す（VAD を通さない）。
pub fn diarize_file(
    audio_path: &Path,
    models_dir: &Path,
    lang: lang::Lang,
    on_progress: Option<&StageProgressFn<'_>>,
) -> Result<diarization::DiarizationResult> {
    // 1) モデル確保（segmentation は tar.bz2 展開、embedding は単体 onnx）
    let dl_cb = download_progress(on_progress);
    let (seg, emb) = ensure_diar_models(models_dir, Some(&dl_cb))?;

    // 2) 原音声を 16k mono へデコード（VAD は通さない）
    report_stage(on_progress, "decode")?;
    let pcm = audio::decode_to_pcm16k_mono(audio_path)?;

    // 3) 話者分離
    report_stage(on_progress, "diarization")?;
    let diarizer = diarization::SherpaDiarizer::new(seg, emb, diarization::DEFAULT_THRESHOLD, lang);
    use diarization::Diarizer;
    let diar = diarizer.diarize(&pcm, 16_000)?;
    cancel::check()?;
    debug_dump_turns(&diar.turns);
    Ok(diar)
}

// 注: ローカル要約（llama.cpp）のオーケストレーションは、whisper.cpp との ggml シンボル衝突のため
// このクレート（whisper を含む）には置けない。別バイナリ（sidecar）/別手段で行う。詳細は新 ADR。
// BYOK 要約は `summarize::{OpenAiSummarizer, AnthropicSummarizer}` が ggml 非依存でそのまま使える。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_reports_ok() {
        assert!(health().contains("ok"));
    }
}
