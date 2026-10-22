# ADR-0037: Optional local live translation

- Date: 2026-09-06
- Status: Accepted for the v0.6.0 preview
- Issue: [#85](https://github.com/daichi-hotta/mojiroku/issues/85)

> Lifecycle and persistence are superseded by [ADR-0038](ADR-0038_Background_model_downloads_and_saved_live_translations.md). The measurements below describe v0.6.0 unless stated otherwise.

## Context

Meeting mode needs English/Japanese translation beside its original live captions.
Caption drafts can change every 3.5-second tick. Recording must continue if translation
is slow, unavailable, cancelled, or fails. Saved transcripts and summaries are separate.

## Decision

Use Qwen3.5-9B Q4_K_M through a bounded, one-shot `mojiroku-llm --translate` invocation.
The model's chat template and an empty thinking block produce only a translation.
Keep llama in the sidecar, isolated from Whisper's ggml symbols (ADR-0007).

Translation is off by default. The meeting panel exposes Translate, an explicit target
language, Turn off, progress, and Retry. It is a temporary preview, cleared on disabling,
target changes, leaving the meeting screen, or stopping recording. It is never saved or
exported. An unchanged caption is translated once; revised text replaces its old result.
Same-language input is requested unchanged. New controls remain English in both UI locales
under the current authoring constraint; localization is deferred.

### Resources and lifecycle

- Require at least 16 GiB of detected physical RAM before starting translation. Unknown
  memory also disables this preview. This is a conservative floor, not a measurement of
  responsiveness on every 16 GB Mac. The measured device has 128 GiB.
- Download 5,680,522,464 bytes only after translation is enabled and a caption is available.
  Store `translation-Qwen3.5-9B-Q4_K_M.gguf` independently of summary selection, so downloading
  translation cannot silently change the existing summary model. It may duplicate weights
  already downloaded for summaries.
- Pin the Unsloth revision `3885219b6810b007914f3a7950a8d1b469d598a5` and verify size plus
  SHA-256 `03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8` before atomic installation.
  A per-process verified-file stamp avoids hashing 5.68 GB for every caption.
- Downloads use unique owned partial files, bounded buffers, cancellation checks, and
  10-second connection / 5-second read timeouts. Activity cancellation stops the download;
  caption revisions let the download finish. UI progress is throttled to 100 ms plus completion.
- One inference in flight, at most four recent pending captions, at most 80 visible rows.
  Older pending captions are marked skipped. Inputs above 1,024 UTF-8 bytes are visibly
  rejected rather than truncated. Context is 2,048 tokens; generation is capped at 512 tokens.
- Live Whisper loading/inference now reserves `HEAVY_ML_JOB` atomically. Translation uses
  `acquire_heavy_job`, including its queued event. Capture/spooling never acquire this permit.
  Busy live ticks retain the existing bounded-tail behavior and may omit preview speech;
  the recorded audio is still available for final transcription.
- Each caption has a session-local ID. Native epochs, monotonically increasing request IDs,
  and exact source-text matching reject stale results and progress. Cancellation that arrives
  before request registration is remembered. Target change/stop cannot cancel a newer session.
- Prompt files are unique, mode 0600, and removed on completion/error. Sidecars have a
  30-second inference timeout and 16 KiB output cap. Cancel/timeout kills the child and drains
  events through process termination before releasing the heavy-job permit. The model unloads
  with the child; no idle translation model remains resident alongside later ML jobs.
  The sidecar also polls its parent every 200 ms and uses `_exit` if the app terminates,
  avoiding concurrent C/C++ exit handlers while inference is still active.

## Evidence and alternatives

Measured on Apple M4 Max, 128 GiB, macOS 26.6.2, Metal enabled. Eight parallel FLEURS
sentence pairs (16 audio recordings) were decoded by the product's turbo/greedy/Auto/VAD
pipeline and translated by the product sidecar. Selection and commands are in
[`eval/translation`](../../eval/translation/README.md). No hand-transcribed labels.

| Candidate | Median process time | Maximum | Peak sidecar RSS | Decision |
|---|---:|---:|---:|---|
| Qwen3.5-4B Q4_K_M | 0.76 s | 0.97 s | 3.01 GB | Reject: wrong technical term and a mistranslated fabric-heating warning |
| Qwen3.5-9B Q4_K_M | 1.07 s | 1.51 s | 5.95 GB | Select: corrects both observed translation errors |

These timings include process/model loading with warm OS caches. A separate 4B cold run
needed 9.2 seconds for its first line, then 0.5–0.6 seconds. Cold launch, download, hashing,
and contention are not represented by the warm median; the UI shows those waits.
The practical target is to finish a short warm caption within the 3.5-second live tick on
the measured Mac. This is not a latency guarantee across hardware.

A 38.22-second real-time replay through `live_stt::run_worker`, using its real SharedPcm
buffer and the actual sidecar under the shared semaphore, produced four translations in
both directions. With 9B, line times were 0.75–0.87 seconds and maximum capture scheduling
lateness was 11 ms. This test exercises the live inference path, not microphone hardware
or ScreenCaptureKit. The first sandboxed run crashed in Metal allocation; the unchanged
test passed with GPU access. UI verification separately replays public captions through
the real meeting components, with mocked IPC, and tests turn-off and target changes.

ASR errors remain visible in translations: for example, one live Japanese caption
misrecognized the periodic table as religion, and the English translation preserved that
incorrect source. This sample supports a preview choice, not a general accuracy score.
The original caption remains available for comparison. No translation model-tier changes
are made to summaries; the 4B summary candidate remains unadopted.

Early screening also rejected the 2B candidate (missing a quantity) and the existing
Qwen2.5-7B candidate (Chinese text in Japanese output). Apple Translation's direct
low-latency session requires newer macOS and language-pack provisioning; the tested language
pair was supported but not installed. Specialist gated or geographically restricted
models were not adopted. No gated license was accepted.

## Validation and limits

Tests cover queue bounds, caption revisions, stale begin/result/progress, target switches,
stop/restart, unavailable memory, retry, listener disposal, pre-registration cancellation,
actual child killing/reaping, parent-death cleanup, output bounds, prompt cleanup, shared permits, and verified
download cancellation/cleanup. The real-audio test is explicitly opt-in so CI does not
require model downloads or audio hardware.

Hardware capture, final saved transcription, summary, playback, export, and signed-release
installation/update checks remain separate release gates. Translation does not alter their
formats, model selection, or persisted jobs. This decision does not complete #87's
unconfirmed affected-recording check.

Model license: [Apache-2.0, Alibaba Cloud](https://huggingface.co/Qwen/Qwen3.5-9B/blob/main/LICENSE).
The app downloads the quantized weights at runtime; attribution is recorded in NOTICE.

The parent-death check passed three times with an explicit watcher marker (135–150 ms
until the child was reaped), with no new crash reports. Merely observing disappearance
is insufficient: the initial `std::process::exit` version crashed during concurrent teardown.
The dedicated reproduction now distinguishes that failure from the intended emergency exit.

During native recording validation, the model answered an English greeting instead of
preserving it when English output was selected. The sidecar now frames source text as a
caption and asks for one of two explicit responses: `UNCHANGED`, or `TRANSLATION` followed
by translated text. When the model chooses `UNCHANGED`, the host copies the source exactly.
This avoids asking the model to reproduce a same-language caption without paraphrasing.
Malformed responses fail visibly rather than exposing protocol text as a translation.
A translated body equal to `UNCHANGED` remains literal text because it follows the separate
`TRANSLATION` header; the parser does not confuse that payload with the control response.

The real-model regression combines eight synthetic English greetings/questions/instructions
with 16 public English/Japanese ASR captions. The baseline preserved 9/24 exactly; the new
protocol preserved 24/24. Three parser tests cover exact copies, marker-like translated
payloads, and invalid formats. Language classification and translation quality remain model
decisions, so these results are a focused regression rather than a universal accuracy claim.


A llama.cpp grammar enforces the control header during sampling; a prompt alone sometimes
omitted it. After `TRANSLATION` and its newline, a fresh normal sampler handles the body.
This avoids per-token grammar filtering for ordinary translation text and keeps control
header tokens out of the body's repetition history. `LlamaSampler::sample` already accepts
the selected token, so the translation loop must not call `accept` again: double acceptance
corrupts grammar state and aborts the sidecar. The real-model regression catches that failure.

The corrected sampler passed all 24 exact-copy cases and returned valid translated responses
for all 16 cross-language public captions. This is not a translation-accuracy score.
The 16 translations took a median of 1.30 seconds and a maximum of 1.80 seconds on the same
M4 Max/128 GiB Mac. The initial model-selection timings above predate this response-protocol
change. They are not the corrected decoder's latency figures.

Caption delimiters are selected per input: append underscores to the tag name until neither
opening nor closing delimiter appears in the source. The system prompt names those exact
delimiters, and the original text is preserved without escaping. This prevents literal markup
such as `</caption>` from colliding with the wrapper. A regression test checks both delimiter
uniqueness and exact source preservation; bypassing the collision check makes it fail.
Four synthetic delimiter cases retained the trailing sentence in real-model checks, including
literal closing-tag text in a markup explanation. The 24 copy and 16 cross-language checks
above were rerun with this framing. Delimiters are a prompt boundary, not a guarantee that
the model cannot follow an instruction embedded in speech.
