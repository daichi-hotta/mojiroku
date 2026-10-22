# ADR-0033: Evaluate beam search before changing the default

- Date: 2026-09-05
- Status: Accepted
- Related: [#76](https://github.com/daichi-hotta/mojiroku/issues/76),
  [#77](https://github.com/daichi-hotta/mojiroku/issues/77)

## Context

The transcription-accuracy plan proposes beam search for recordings and imported
files, while live transcription needs to retain greedy decoding. Both paths use
the same Whisper engine. A global sampling change would also slow live ticks.
An expected accuracy improvement is insufficient reason to change the default.

## Decision

Keep **greedy decoding as the file and live default**. Provide an explicit
`DecodingStrategy` with greedy and beam width 5 for controlled offline evaluation.
The file/recording paths share `FILE_DECODING`; the live `SttEngine::transcribe`
entry point selects greedy independently. The evaluation CLI uses
`transcribe_file_with_decoding`, which runs the same audio decode, VAD, FFI guard,
and timestamp mapping as the product. Existing public file APIs keep their shape.
There is no new UI setting or background ML job.

Use `eval/stt/` as a reproducible public-audio sanity check. Download a pinned
FLEURS test subset with existing transcripts, verify corpus/model hashes, score
Japanese CER and English WER, and record timing and input provenance. Keep audio
and raw results local. This is read speech; it does not establish meeting quality.

## Evidence

The [harness README](../../eval/stt/README.md#baseline) records the commands,
normalisation, corpus/model revisions, hardware, and full aggregate table.
On 20 Japanese and 20 English recordings (automatic language detection):

| Metric | Greedy | Beam 5 |
|---|---:|---:|
| Japanese CER | 29 / 1,058 = 2.74% | 33 / 1,058 = 3.12% |
| English WER | 32 / 467 = 6.85% | 32 / 467 = 6.85% |
| Japanese pipeline seconds | 17.78 | 19.80 |
| English pipeline seconds | 18.23 | 19.39 |

The sample provides no aggregate accuracy gain to justify the added time. It does
not prove that beam search cannot help other recordings. Revisit the file default
if broader evaluation or real-use evidence demonstrates a worthwhile tradeoff.

## Alternatives and consequences

- **Enable beam 5 for all file jobs now:** rejected on the measured sample; it
  increases latency without demonstrating better aggregate accuracy.
- **Change the shared decoder globally:** rejected because it also affects live
  transcription. The two entry points must keep independent policy choices.
- **Remove beam support after the experiment:** would make future comparisons
  require source edits. Keep the explicit core/CLI option, with no settings UI.
- **Use a separate Python inference engine:** would not measure the app's VAD and
  Whisper configuration. Python only downloads, orchestrates, and scores.

The harness uses no manually labelled recordings. Small public-corpus scores are
a sanity check; post-release use still decides product quality. Larger subsets,
meeting audio, and mixed-language evaluation remain possible follow-ups, not
claims established by this measurement.
