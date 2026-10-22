# 0035. Adjust very quiet audio for VAD analysis without changing Whisper's input

- Status: implemented; release validation pending
- Date: 2026-09-05
- Related: [Issue #87](https://github.com/daichi-hotta/mojiroku/issues/87), ADR-0008, ADR-0031, ADR-0032

## Context

A local reproduction of the live-versus-final content gap traced the difference to VAD input level. Replaying the v0.5.6 STT implementation produced 14 committed live-preview segments and 3 final segments. The final output matched the stored transcript, including timestamps. Whether this candidate is the reporter's intended recording remains unconfirmed. Audio, transcript text, recording IDs, and identifying details remain local.

In v0.5.6, a short window with no detected speech fell back to raw PCM, while a whole-track call with some detected speech retained only those spans. Quiet speech could therefore appear in the live preview and disappear in the final result. ADR-0031 removed the raw-silence fallback to prevent hallucinations, but did not address speech rejected due to low input level. Replaying the newer implementation produced about three committed segments in both paths; equalizing the paths alone did not restore speech.

Controlled tests on public FLEURS speech distinguish lost words from extra model output. The first three English and Japanese fixtures selected by the existing FLEURS harness were attenuated to RMS 0.0003, then quantized to 16-bit PCM. With the same turbo model, automatic language selection, and greedy decoding, applying 16x gain **only to VAD analysis** gave:

| Metric | Original VAD input | Leveled VAD input |
| --- | ---: | ---: |
| English word errors | 41 / 55 | 10 / 55 |
| Japanese character errors | 177 / 203 | 3 / 203 |

At RMS 0.0015, the same six fixtures had unchanged error counts. These small, artificially attenuated read-speech samples establish the failure mechanism; they are not a general meeting-quality benchmark. Counts of segments in private recordings are diagnostic evidence, not accuracy scores.

## Decision

Prepare a separate analysis buffer before Silero VAD:

1. Compute RMS for each one-second block, including a partial final block. Ignore exactly zero blocks when estimating level so digital-silence padding cannot hide a short utterance.
2. Use the 90th percentile of those block levels. An isolated loud sound should not prevent adjustment of an otherwise quiet recording.
3. If the reference level is at least 0.01 RMS (-40 dBFS), borrow the original input. Otherwise target 0.05 RMS, with gain capped at 16x (about 24 dB). Clamp the analysis samples to [-1, 1].
4. Run the existing Silero thresholds on that analysis buffer. Release the buffer before allocating the filtered Whisper input.
5. Extract speech from the **original PCM**, using the detected sample indices. Keep the existing gap insertion, timestamp mapping, and empty-transcript behavior when no speech is found.

The shared STT path applies this to both live and offline inference. The saved WAV, playback, Whisper sample amplitudes, and diarization input are unaffected. No persisted setting or schema change is needed. `vad_spans_cli` uses the same preparation by default and accepts a final `raw` argument for an unadjusted comparison; its RMS measurements still describe the original audio.

The live worker must allow quiet tails to reach this shared path. With a VAD model present, its early gate skips only digital silence (all-zero samples), leaving speech classification to VAD. Without a VAD model, it retains the existing RMS 0.001 guard against low-level noise reaching Whisper directly. The former unconditional RMS guard rejected the RMS 0.0003 regression level before VAD could run. Tail duration limits, draining, and heavy-job yielding are unchanged.

When the live worker configures VAD, it also enables `WhisperStt::with_required_vad()`. A VAD initialization or inference error then returns an error to the worker, which skips that preview attempt without affecting recording. This prevents a corrupt, unreadable, or subsequently removed model from silently falling back to raw Whisper after the early RMS gate has admitted quiet audio. Other STT callers retain their existing fallback policy unless they explicitly require VAD.

Failed live inference drains paired pending buffers back to the existing 14-second tail limit. Persistent VAD errors therefore cannot accumulate the entire meeting in the live worker; a repeated-failure test covers the bound and track alignment.

The diagnostic CLI recognizes a trailing `raw` or `leveled` mode independently of the optional numeric parameters: `vad_spans_cli <audio> <models_dir> raw` uses all default thresholds.

The cutoff, target, percentile, and cap are a bounded initial policy, not universally optimal values. Quiet inputs with varied speaker levels and background speech still need real-use validation. VAD can admit background speech; raising the input level does not distinguish intended speech from an intelligible background conversation.

## Regression verification

Unit checks cover bounded gain, a loud outlier, digital silence, partial blocks, unchanged original samples, and borrowing normal-level input. The real-model test checks that VAD preserves the words the decoder can hear in a pinned public English fixture and still rejects 60 seconds of digital silence. Punctuation and case are excluded from the word comparison.

Live-worker regression checks cover a tail below the old RMS cutoff, digital silence with and without VAD, and retention of the RMS guard when VAD is unavailable.

The real-model regression also requires errors for corrupt, removed, and unconfigured VAD when VAD is mandatory. CLI tests cover both mode names with zero through three numeric options and the default mode; run them with `cargo test -p mojiroku-core --example vad_spans_cli`.

Verification of the implemented policy reproduced the attenuated-speech results above. The six unattenuated public fixtures and both tracks of an earlier meeting fixture produced identical transcripts before and after the change. The local candidate produced 14 segments instead of 3; its accuracy still requires listening-based confirmation. Generated digital silence and quiet Gaussian noise with a loud transient both produced zero segments. Bypassing the new preparation made the real-model regression fail for lost words; restoring it made the regression pass.

Fixture: FLEURS `en_us/17931113498862338153.wav`, revision `70bb2e84b976b7e960aa89f1c648e09c59f894dd`, SHA-256 `697876fbd65b56e578f94a0eed8fa23ef2f0afbb149c83f402135448abed344e`. The test verifies the checksum before reading speech. It uses the existing local turbo and Silero model files and downloads nothing.

```sh
MOJIROKU_TEST_SPEECH_WAV=/path/to/fleurs/en_us/audio/17931113498862338153.wav \
MOJIROKU_TEST_MODELS=/path/to/models \
cargo test -p mojiroku-core --lib \
  vad_keeps_attenuated_public_speech_and_rejects_silence -- --ignored
```

Run this opt-in check with normal GPU access. The ordinary core unit tests do not require the fixture or models. Private recording content must never be included in regression fixtures or test failure output committed to the repository.

## Alternatives

- Restore the raw-PCM fallback or disable VAD: rejected because silence hallucinations were already reproduced in ADR-0031.
- Lower the VAD probability or duration thresholds globally: not needed for the demonstrated input-level problem; it would also alter normal-level behavior.
- Always use short inference windows: replaying current code did not restore the missing speech. Windowing alone does not address low VAD input level.
- Normalize the saved audio or Whisper input: unnecessary. Raw Whisper decoding already recovered the public reference words at the tested levels.
- Use absolute peak normalization: a brief loud transient in an otherwise quiet track can prevent useful gain.
