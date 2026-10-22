# 0036. Opt into language re-detection at speech pauses

- Status: implemented; release validation pending
- Date: 2026-09-06
- Related: [#79](https://github.com/daichi-hotta/mojiroku/issues/79), [v0.6.0 #89](https://github.com/daichi-hotta/mojiroku/issues/89), ADR-0031, ADR-0032, ADR-0035

> Language selection is updated by [ADR-0039](ADR-0039_Restrict_automatic_recognition_to_Japanese_and_English.md): Auto and mixed windows now consider Japanese and English only.

## Context

The vendored `whisper_full_with_state` detects language at offset zero before its decoding loop (`vendor/whisper-rs-sys/whisper.cpp/src/whisper.cpp`). It then constructs one language/task prompt for the call. The application passes the whole VAD-filtered track to that call.

Four constructed public recordings reproduced the resulting failure: Japanese and English passages after a switch were omitted or rendered in the other language. The fixtures join existing FLEURS clips in both directions, including switches within and across Whisper's decoding windows. Their references come from the corpus; no manual labels or private recordings are used.

An initial-prompt-only change would not take effect under the current `n_max_text_ctx = 0` history guard. Changing that guard would reopen the repetition behavior addressed by ADR-0032. Splitting on every short VAD gap also creates unnecessary calls and less context. The measured prototype instead groups speech across short gaps and re-detects language after longer pauses.

## Decision

Add **Japanese + English (slower)** as an explicit transcription-language choice. Store it as `transcribe_language = "mixed"`; existing settings and jobs retain their previous behavior. `JobParams.stt_lang` already captures the effective choice at enqueue time, so no schema change is needed. The core consumes `MIXED_LANGUAGE_MODE` before configuring Whisper: this application marker must never be passed to `FullParams` as a language code.

For this mode:

1. Run the existing Silero analysis and padding policy to find original-time speech ranges. Quiet-input adjustment remains VAD-only.
2. Split at the midpoint of pauses of approximately 750 ms or longer before padding. With the existing 200 ms padding on either side, this is a 350 ms gap between padded ranges. Shorter gaps stay within one window.
3. Reuse the loaded Whisper model, but create a fresh decoding state per window with automatic language detection and the existing rolling-history guard. Each window uses the real STT path, including VAD and timestamp remapping.
4. Offset window timestamps back into the original recording. Map each window's progress into the whole recording so progress cannot restart at zero between calls.
5. Require successful VAD for both the initial segmentation and every window. Missing or failed VAD returns an error; no partial transcript or raw-audio fallback is substituted. Live workers retain their bounded error-retry behavior from ADR-0035.

The mode travels through the existing language-setting snapshot for queued file, mic, and dual-track meeting jobs and for live preview. Offline jobs keep their selected model and decoder; live continues using turbo and greedy. **Auto-detect remains the fast default.** The new mode adds inference calls and is optional rather than a change to every recording.

## Verification

Measured on Apple M4 Max, 128 GiB RAM, macOS 26.6.2, release build with Metal, turbo q5_0 and Silero v5.1.2. The four switching fixtures reuse six unique FLEURS clips; they are not independent meeting samples.

| Constructed recording | Duration | Auto pipeline | Mixed pipeline | Retention gate: Auto / Mixed |
| --- | ---: | ---: | ---: | --- |
| Japanese → English → Japanese | 48.16 s | 1.35 s | 2.38 s | fail / pass |
| English → Japanese → English | 34.54 s | 0.88 s | 2.29 s | fail / pass |
| Longer Japanese → English → Japanese | 70.96 s | 1.59 s | 3.84 s | fail / pass |
| Longer English → Japanese → English | 72.08 s | 1.47 s | 4.10 s | fail / pass |

The mixed calls took 2.39x the aggregate pipeline time. Two constructed single-language controls also passed. Full large-v3 passed all six constructed fixtures in mixed mode; turbo remains the default.

Across the four switching fixtures, Japanese errors changed from 265/568 to 9/568 characters, and English errors from 95/143 to 17/143 words. These totals reuse the source clips across fixtures. The existing normalization is unchanged: for example, `A.D.` versus `AD` contributes word errors. One mixed English passage also loses an article compared with its isolated decoding; the result is not a claim of perfect recognition.

The retention gate compares each continuous language block with the same model's isolated-clip errors. It allows the larger of two additional error units or 5% of the reference units, rejects wholly missing/wrong text, and requires valid ordered timestamps with no segment crossing a language boundary. Scores remain unmodified. This checks for lost passages while allowing small context-dependent wording differences; it is not a general quality target.

The original `develop` implementation and the modified **Auto** path produced **identical transcripts on all 40 existing single-language corpus samples**. Rust tests cover pause boundaries, sample coverage, monotone aggregate progress, settings/job snapshots, and the existing VAD safety cases. The UI was inspected in a browser preview with a simulated settings backend; native release validation remains separate.

Baseline commit: `45dfae5118709e8ca643dc6ded197f5ac9206e9e`, plus this change for the candidate. Measured candidate CLI SHA-256: `eca82952eba3e95ca1277e3f0f95ec7281bbf8d62ed839877d6b62b71e61d83f`. Constructed manifest SHA-256: `e67d62a4edc26c0bbd5b51da02c977c7f5d59871397733d418eacbec44539333`. The harness also records model, source, runner, and scorer hashes.

Reproduction commands and data handling are in [the evaluation guide](../../eval/stt/README.md#mixed-language-recordings).

## Limits

- Detection changes at speech pauses, not at every word. Switching without a clear pause, very short utterances, overlapping speakers, and natural meeting noise still need real-use evaluation.
- The 750 ms boundary is an initial policy, not an optimized multilingual detector. The synthetic fixtures contain one-second inserted pauses plus the original clip silence.
- There is an initial VAD scan followed by per-window VAD. This intentionally reuses the established STT path; optimization can follow measured need.
- These measurements do not establish performance on 16 GiB Macs or long real meetings. They do not complete the affected-recording validation for #87 or the live-translation work in #85.
