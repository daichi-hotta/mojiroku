//! Evaluation CLI for two-track meetings: STT on the mic and system tracks, the mic crosstalk
//! filter (ADR-0048), then the product's track merge. Speaker separation is skipped: the text
//! and the crosstalk decision do not depend on it, and it would double the run time.
//! Usage: meeting_cli <mic.wav> <system.wav> <models_dir> <lang|auto|mixed> <mic_offset_ms>
//!        [default|greedy|beam5] [--model <catalog-file>]
//! Prints one JSON object on stdout with the filtered merge (`transcript`) and the same
//! Whisper output merged without the filter (`unfiltered_transcript`), so one run scores
//! both. Diagnostics stay on stderr.

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let usage =
        "usage: meeting_cli <mic> <system> <models_dir> <lang> <mic_offset_ms> [decoder] [--model f]";
    let (mic_path, system_path, models_dir) = (
        Path::new(args.get(1).expect(usage)),
        Path::new(args.get(2).expect(usage)),
        Path::new(args.get(3).expect(usage)),
    );
    let language = match args.get(4).expect(usage).as_str() {
        "auto" => None,
        other => Some(other.to_string()),
    };
    let mic_offset_ms: i64 = args
        .get(5)
        .expect(usage)
        .parse()
        .expect("mic_offset_ms must be an integer");
    use mojiroku_core::stt::{DecodingStrategy, FILE_DECODING};
    let decoding = match args.get(6).map(String::as_str).unwrap_or("default") {
        "default" => FILE_DECODING,
        "greedy" => DecodingStrategy::Greedy,
        "beam5" => DecodingStrategy::BeamSearch5,
        _ => panic!("decoder must be default, greedy, or beam5"),
    };
    let mut model = mojiroku_core::models::select_whisper_model(None);
    let mut flags = args.iter().skip(7);
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--model" => {
                let file = flags.next().expect("--model needs a catalog file name");
                model = mojiroku_core::models::WHISPER_MODELS
                    .iter()
                    .find(|model| model.file == file)
                    .expect("unknown Whisper model; use a catalog file name");
            }
            _ => panic!("unknown option: {flag}"),
        }
    }
    let options = mojiroku_core::TranscriptionOptions {
        language: language.as_deref(),
        model,
        decoding,
    };
    let on_progress = |stage: &str, done: u64, total: Option<u64>| {
        eprintln!("[{stage}] done={done} total={total:?}");
    };

    let started = std::time::Instant::now();
    let system = mojiroku_core::transcribe_file_with_options(
        system_path,
        models_dir,
        options,
        Some(&on_progress),
    )
    .expect("system track transcription failed");
    let mut mic = mojiroku_core::transcribe_file_with_options(
        mic_path,
        models_dir,
        options,
        Some(&on_progress),
    )
    .expect("mic track transcription failed");
    let unfiltered_mic = mic.clone();
    // Same condition as transcribe_meeting_dual_track_with_options.
    let report = if system.segments.is_empty() {
        None
    } else {
        mojiroku_core::crosstalk::filter_mic_track(&mut mic, mic_path, system_path, mic_offset_ms)
    };
    let lang = mojiroku_core::lang::Lang::from_code("ja");
    let merge = |mic, system| {
        mojiroku_core::merge::merge_tracks(mic, system, Vec::new(), lang, mic_offset_ms).0
    };
    let unfiltered = merge(unfiltered_mic, system.clone());
    let transcript = merge(mic, system);
    println!(
        "{}",
        serde_json::json!({
            "transcript": transcript,
            "unfiltered_transcript": unfiltered,
            "pipeline_seconds": started.elapsed().as_secs_f64(),
            "whisper_model": model.file,
            "crosstalk": report.map(|r| serde_json::json!({
                "own_voice_dbfs": r.own_voice_dbfs,
                "dropped": r.dropped,
                "kept": r.kept,
            })),
        })
    );
}
