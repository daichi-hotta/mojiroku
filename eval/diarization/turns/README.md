# Turn order and attribution (Issue #65)

Checks whether the transcript shows two people's utterances **in the order they
spoke, with the right speaker**, especially around quick hand-offs and short
replies. The real recordings used elsewhere in `eval/diarization/` have
hand-marked intervals accurate to a few seconds, which cannot show a mistake at
a single turn change.

`make_dialogue.py` synthesises a two-speaker dialogue (Japanese and English) with
the macOS `say` voices, so every turn's start, end, speaker and text are exact.
It is **synthetic speech**: clean, no room, no overlap. Use it to see ordering and
attribution around turn changes, not recognition or diarization quality on real
meetings. The script text is original; audio and results stay in ignored `out/`.

## Run (macOS, repository root)

```bash
python3 eval/diarization/turns/make_dialogue.py
cargo build --release -p mojiroku-core --example transcribe_diarize_cli
M="$HOME/Library/Application Support/com.daichi0812.mojiroku/models"
for l in ja en; do
  MOJIROKU_DEBUG_TURNS=eval/diarization/turns/out/$l.turns.json \
    target/release/examples/transcribe_diarize_cli eval/diarization/turns/out/$l.wav "$M" $l \
    eval/diarization/turns/out/$l.pred.json
done
python3 eval/diarization/turns/score.py \
  eval/diarization/turns/out/ja.json eval/diarization/turns/out/ja.pred.json \
  eval/diarization/turns/out/en.json eval/diarization/turns/out/en.pred.json
```

`MOJIROKU_DEBUG_TURNS=<path>` makes the core write the final speaker turns to
`<path>` and sherpa-onnx's raw segments (before consolidation) to
`<path with .raw.json>`, for the file transcription and re-diarization paths.

## Metrics

| Metric | Meaning |
|---|---|
| `straddling` | transcript lines covering two reference turns (≥ 0.2 s each) |
| `mislabelled_time` | share of reference speech whose line has the wrong speaker |
| `turn_order_error` | edit distance of the speaker sequence (consecutive lines merged) ÷ reference turns |
| `short_replies_ok` | turns under 1.2 s that got their own line with the right speaker |
| `text_recall` | share of reference characters found in order (punctuation and spaces ignored) |

## Baseline (develop, 2026-09-25, Whisper turbo, pyannote seg-3.0 + TitaNet)

| | straddling | mislabelled | order error | short replies | recall |
|---|---:|---:|---:|---:|---:|
| ja (58.8 s, 15 turns) | 0 | 1.7 % | 0.13 | 2/3 | 0.955 |
| en (46.1 s, 15 turns) | 0 | 6.8 % | 0.40 | 1/4 | 1.000 |

**Whisper already breaks lines at turn changes** (no line spans two turns), so
splitting lines at diarization turn boundaries would not change these results.
The errors are short replies ("Right.", "Sure.", 「はい」) that are **missing from
sherpa-onnx's raw output**: the surrounding speaker's segment covers them, and
consolidation keeps them there. Fixing that means changing diarization, which
pulls against the brief-speaker rules in ADR-0041/0042 (a one-on-one produced
eight speakers from backchannels), and needs real-meeting evidence first.
