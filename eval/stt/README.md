# Transcription evaluation

Compare greedy decoding and beam search (width 5) through the **real file pipeline**:
audio decode → Silero VAD → Whisper → original-time timestamp mapping. The runner
invokes `transcribe_cli`; there is no separate Python speech model.

This is a **sanity check on read speech, not a meeting-quality benchmark**. FLEURS
does not test overlapping speakers, conversational backchannels, long recordings,
or Japanese/English switching within a meeting. Real-use feedback after release
decides whether the app is better. No manual transcription or labelling is required.

Related: [#76](https://github.com/daichi-hotta/mojiroku/issues/76),
[#77](https://github.com/daichi-hotta/mojiroku/issues/77).

## Data and attribution

[FLEURS](https://huggingface.co/datasets/google/fleurs), by the FLEURS authors
([2022 paper](https://arxiv.org/abs/2205.12446)), is distributed under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). The harness downloads
the Japanese `ja_jp` and English `en_us` **test** archives and their existing
transcripts from revision `70bb2e84b976b7e960aa89f1c648e09c59f894dd`.
Archive and TSV SHA-256 hashes are pinned in `download.py`.

The first download is about 739 MB; extracted samples need additional disk space.
Audio, reference text, and raw predictions stay in ignored `cache/` and `results/`
directories. They are not part of the app or this repository. The downloader copies
selected WAVs unchanged. Scoring normalises the text as described below.

Selection sorts recording filenames by SHA-256 of `mojiroku-stt-v1:<filename>` and
takes the first N **per language**. The default is 20. This is independent of TSV
row order and Python's random-number generator. Sentence IDs can repeat across
speakers, so recording filenames are the unique keys. A larger N extends the same
sample; it does not resample it. The manifest records audio hashes, references,
duration, revision, selection rule, and attribution.

## Run

Use Python 3.10+ (standard library only) and the normal Rust/Apple Silicon build
environment. Commands below run from the repository root. First let the app finish
downloading its default Whisper and Silero models. The runner verifies their hashes
and rejects missing or different models before inference.

```bash
python3 eval/stt/download.py --limit 20
cargo build --release -p mojiroku-core --example transcribe_cli
python3 eval/stt/run.py \
  --manifest eval/stt/cache/fleurs-test-20.json \
  --models "$HOME/Library/Application Support/com.daichi0812.mojiroku/models"
```

Run with Metal GPU access. An agent sandbox that denies the GPU can crash the
bundled Metal backend before transcription starts. Do not disable GPU acceleration
to obtain a nominally comparable timing. Stop other transcription/summary jobs and
avoid builds during the measurement; the harness runs one ML process at a time.

Optional arguments:

- `download.py --limit 100`: 100 recordings per language. Archive downloads are cached.
- `download.py --cache /path`: keep data elsewhere; pass its manifest to the runner.
- `run.py --language-mode reference`: force `ja`/`en` from the dataset. The default
  is **auto**, matching the app's automatic transcription setting. Do not compare
  runs with different language modes as if only the decoder changed.
- `run.py --binary /path/to/transcribe_cli`: use a separately built executable.
- `run.py --output /path/to/new-directory`: choose an output directory; existing
  directories are rejected so previous results cannot be mixed into a new run.
- `run.py --timeout 600`: seconds per recording; errors abort the run.
- `run.py --comparison models`: compare turbo with full large-v3 q5_0, both using
  greedy decoding. The default `--comparison decoders` still compares greedy and
  beam 5 on turbo. Both model files must already be cached for the model comparison.

To inspect one file:

```bash
target/release/examples/transcribe_cli audio.wav /path/to/models auto greedy --json
target/release/examples/transcribe_cli audio.wav /path/to/models auto beam5 --json
```

The existing positional CLI and human-readable output still work. The added fourth
positional argument selects `default`, `greedy`, or `beam5`; an optional `--json`
follows it. JSON includes segments, selected decoder/model names, and pipeline time.
Use `--model ggml-large-v3-q5_0.bin` after the decoder to select full large-v3.
`--model` and `--json` may appear in either order. Omission keeps turbo. Unknown
model names fail rather than silently benchmarking a different model.

To download full large-v3 through the product's checksum-verified path, run one
sample first (the initial download time is excluded from later benchmark runs):

```bash
target/release/examples/transcribe_cli audio.wav /path/to/models auto greedy \
  --model ggml-large-v3-q5_0.bin --json
python3 eval/stt/run.py --comparison models --models /path/to/models
```

## Scoring and timing

- Japanese CER: Unicode NFKC width normalisation, remove Unicode punctuation and
  whitespace, then character edit distance divided by reference characters. The
  long-vowel mark is preserved. Numbers and kanji readings are not rewritten.
- English WER: NFKC, lowercase, remove straight/curly apostrophes within words,
  replace other Unicode punctuation with spaces, then whitespace-delimited word
  edit distance divided by reference words. Numbers are not expanded into words.
- Join English segments with a space and Japanese segments without one. Each
  recording is scored separately so errors cannot align across recordings.
- Aggregate by **total errors / total reference units**, not mean per-file rates.
  Empty predictions count as deletions. An empty normalised reference fails the
  run. Insertions can push CER/WER above 100%; rates are not clipped.
- Run one excluded warm-up per decoder, then alternate which decoder goes first
  for each recording. Both see exactly the same input and language setting.
- `pipeline_seconds` includes model loading, audio decode, VAD, and Whisper, with
  models already cached. `process_seconds` additionally includes executable launch
  and exit. Neither includes compilation or corpus download. RTF is total pipeline
  seconds / total audio seconds; lower is faster. This is not decoder-only timing.

`metadata.json` records model, binary, Rust source, manifest, scorer, and runner
hashes, Git revision/dirty state, platform, and language mode. `recordings.jsonl`
contains references, hypotheses, segments, errors, and timing for every call.
Per-call `.log` files keep stderr. `summary.json` is written **only if all pairs
succeed**. A VAD filtering failure rejects the run instead of silently scoring raw
audio. These outputs remain local; only aggregate numbers belong in public reports.

The normalisation identifier is `nfkc-punctuation-v1`; changing these rules requires
a new identifier and a fresh baseline. These scores are not directly comparable to
published FLEURS scores that use another normaliser, model, or language hint.

## Checks

```bash
python3 -m unittest discover -s eval/stt -p 'test_*.py' -v
cargo test -p mojiroku-core
```

Tests cover edit counts, width/punctuation handling, empty output, word boundaries,
weighted aggregation, and deterministic selection. Rust checks pin the effective
Whisper search width and the rolling-history guard for both decoder choices.

## Baseline

Measured 2026-09-05 on Apple M4 Max, 128 GiB RAM, macOS 26.6.2, release build with
Metal. Default large-v3-turbo q5_0, Silero v5.1.2, automatic language detection,
rolling text history disabled (ADR-0032), 20 recordings per language. Audio totals:
Japanese 260.40 s, English 199.66 s. One measured call per recording/decoder.

| Language | Decoder | Errors / reference units | CER / WER | Pipeline seconds | Process wall seconds |
|---|---|---:|---:|---:|---:|
| Japanese | Greedy | 29 / 1,058 characters | 2.74% | 17.78 | 20.91 |
| Japanese | Beam 5 | 33 / 1,058 characters | 3.12% | 19.80 | 23.40 |
| English | Greedy | 32 / 467 words | 6.85% | 18.23 | 20.96 |
| English | Beam 5 | 32 / 467 words | 6.85% | 19.39 | 21.41 |

Beam search improved / tied / worsened per-recording error counts on 1 / 16 / 3
Japanese recordings and 1 / 18 / 1 English recordings. It took 11.4% more Japanese
pipeline time and 6.3% more English pipeline time, without an aggregate accuracy
gain on this sample. **Keep greedy as the file default; live stays greedy.**
This small sample does not establish that beam search is worse in general, and
these absolute timings are not estimates for 16 GiB Macs or long meetings.

Baseline provenance: base commit `d026b91904c722437aabb1a31df85143863b8ce0` plus
this change; measured executable SHA-256
`39cab9cffd4ba2eba17f0c62b313fece4c452bbc86dcb07cb976764d84dc8b19`.
The decoder configuration retains whisper.cpp's existing temperature/fallback
defaults. Model checksums are in `run.py`; corpus checksums are in `download.py`.
The full default decision is in
[ADR-0033](../../docs/decisions/ADR-0033_Evaluate_beam_search_before_changing_the_default.md).

## Full large-v3 comparison

Measured 2026-09-05 on the same fixed 20-recording subsets and hardware above, using
greedy decoding and automatic language detection for both models. The initial
1,081,140,203-byte full-model download passed the product checksum check; all measured
calls reused cached models. Full-model SHA-256:
`d75795ecff3f83b5faa89d1900604ad8c780abd5739fae406de19f23ecd98ad1`.

| Language | Model | Errors / reference units | CER / WER | Pipeline seconds | Process wall seconds |
|---|---|---:|---:|---:|---:|
| Japanese | Turbo | 29 / 1,058 characters | 2.74% | 17.96 | 20.95 |
| Japanese | Full large-v3 | 29 / 1,058 characters | 2.74% | 28.61 | 32.54 |
| English | Turbo | 32 / 467 words | 6.85% | 20.73 | 24.95 |
| English | Full large-v3 | 27 / 467 words | 5.78% | 30.70 | 36.02 |

Full large-v3 made five fewer English word errors and the same aggregate number of
Japanese character errors. It used 1.59× Japanese and 1.48× English pipeline time.
Keep it **opt-in**, with turbo as the default; this small read-speech sample does
not establish a general accuracy advantage or meeting quality. The default/tier
decision still depends on real-use feedback. See
[ADR-0034](../../docs/decisions/ADR-0034_Offer_full_Whisper_large_v3_for_offline_transcription.md).

Model-comparison provenance: base commit `cf0452d` plus this change; measured
executable SHA-256 `5c5058146c7b4cff36c5b056c12112aee637313c5854226c14c14aaa3a06661a`.

## Private meetings with an exported transcript

`reference.py` compares a meeting saved in the app with the transcript exported from the
meeting tool (for example Zoom's `.vtt`/`.txt`). Both stay on the Mac. **The report is
numbers only**: CER with substitution/deletion/insertion counts, error categories,
anonymous reference speakers (`R1` spoke most), mojiroku speaker ids, 5-minute windows,
clock offset, segment diagnostics, and mic/system loudness. It never prints transcript
text, names, the reference file's content, or other recordings' titles; an unknown file
format is reported as line shapes (`[JJ] 99:99:99`). Exceptions print only their type and
line number. Use it on confidential meetings without opening them.

```bash
APP="$HOME/Library/Application Support/com.daichi0812.mojiroku"
# The saved transcript (read-only DB), plus loudness and a crosstalk simulation. The app
# already drops leaked mic segments since ADR-0049, so the simulation only shows the
# filter's effect on meetings transcribed before it; use the rerun below for newer ones.
python3 eval/stt/reference.py saved --db "$APP/mojiroku.db" --reference ~/Downloads/meeting.vtt \
  --title "<recording title>" --recordings "$APP/recordings"
# Fresh runs on the saved audio, filtered and unfiltered (ADR-0049) from one call each:
cargo build --release -p mojiroku-core --example meeting_cli
python3 eval/stt/reference.py rerun --db "$APP/mojiroku.db" --reference ~/Downloads/meeting.vtt \
  --title "<recording title>" --recordings "$APP/recordings" --models "$APP/models" \
  --meeting-binary target/release/examples/meeting_cli --variants full-greedy,turbo-greedy
```

- The reference is usually another automatic transcript. Treat CER as disagreement, and
  read the categories: fillers and kana/kanji spelling differ by style, not accuracy.
- Only the time both cover is scored: mojiroku text after the export stops, and reference
  text from before the recording started or after it stopped, are left out and counted.
  The clock offset comes from matched characters, so wall-clock exports (13:02:11) and
  relative ones both work. `--no-clip` scores everything.
- Alignment uses unique character n-gram anchors and exact edit distance between them, so
  a one-hour meeting aligns in well under a second. The count is an upper bound on the true
  edit distance; `unaligned_units` reports anything no anchor could place.
- `rerun --binary transcribe_cli` transcribes each track separately and merges them like
  `merge_tracks`; `--meeting-binary meeting_cli` runs the product's crosstalk filter and
  merge too. Speaker separation is skipped in both. Transcripts stay in memory; nothing is
  written except the optional `--json-out` aggregate report. Keep that under the ignored
  `results/` folder and never commit it.

## Mixed-language recordings

The optional **Japanese + English (slower)** setting re-detects language at speech
pauses. `auto` keeps the original fast path; `mixed` selects the new mode in
`transcribe_cli`. See [ADR-0036](../../docs/decisions/ADR-0036_Opt_in_language_switching_at_speech_pauses.md).

`mixed.py` builds four switching fixtures and two single-language controls from the
first three existing FLEURS samples per language. It preserves float PCM samples
and inserts one second of silence between clips. The resulting language boundaries
and reference text come from composition and corpus labels. This is a constructed
read-speech check, not evidence about natural code-switching or overlapping meetings.

```bash
python3 eval/stt/download.py --limit 20
python3 eval/stt/mixed.py prepare
cargo build --release -p mojiroku-core --example transcribe_cli
python3 eval/stt/mixed.py run \
  --models /path/to/models \
  --output eval/stt/results/mixed-turbo
```

Use a fresh output directory for each run. Source copies, composed audio, references,
and raw transcripts stay under ignored `cache/` and `results/` directories.
The manifest pins source and composed audio hashes and retains corpus attribution.
The run records model/binary/source/scorer hashes, warms up each mode, alternates
mode order, and executes one inference process at a time with Metal access.

The default gate requires `mixed` to pass every fixture. A mode passes only when:

- Each continuous language block retains nonempty text and has fewer errors than
  deleting the entire reference.
- Errors stay within the isolated-clip baseline plus the larger of two additional
  units or 5% of the reference units. `--extra-units` and `--extra-rate` expose these
  tolerances. CER/WER normalization and raw error counts are unchanged.
- Timestamps are valid and ordered, and no segment crosses a language boundary.

Adjacent clips in the same language are scored as one block, so a valid segment
spanning two same-language clips is not penalized as missing text. Isolated sources
are decoded with the same model and automatic detection before comparison. This
avoids mistaking existing errors, such as `A.D.` versus `AD`, for switching losses.
It still reports those errors; an English fixture gains one missing article in
mixed mode compared with its isolated result.

To check the optional full model once it is downloaded:

```bash
python3 eval/stt/mixed.py run --models /path/to/models \
  --model ggml-large-v3-q5_0.bin --modes mixed \
  --output eval/stt/results/mixed-full
```

To verify that ordinary Auto behavior remains unchanged, build `transcribe_cli`
from the comparison baseline in a separate worktree, then compare the existing
40-recording corpus. The check compares complete transcript objects, including
timestamps, and fails if any differ:

```bash
python3 eval/stt/mixed.py compare-auto \
  --baseline-binary /path/to/baseline/transcribe_cli \
  --models /path/to/models \
  --output eval/stt/results/auto-regression
python3 -m unittest discover -s eval/stt -p 'test_*.py' -v
```

Measured against `45dfae5` on Apple M4 Max, 128 GiB RAM, macOS 26.6.2:

| Check | Auto | Mixed |
| --- | --- | --- |
| Four switching recordings | 0/4 pass | 4/4 pass |
| Two constructed single-language controls | 2/2 pass | 2/2 pass |
| Pipeline time for the four switching recordings | 5.28 s | 12.61 s |
| Existing 40-recording corpus versus baseline Auto | 40/40 identical transcripts | Not the default path |
| Full large-v3 on the six constructed fixtures | Isolated-source baselines only | 6/6 pass |

The switching recordings total 225.74 seconds and reuse six source clips. The
2.39x measured cost is why the new mode is opt-in. Word-level switches without a
clear pause remain outside what these fixtures establish.
