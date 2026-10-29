# ADR-0048: Drop meeting mic segments that are leaked remote audio

- Date: 2026-10-29
- Status: Accepted
- Related: ADR-0017 (meeting system-audio capture), ADR-0031 (VAD gaps and silent input),
  ADR-0035 (VAD level adjustment), `eval/stt/reference.py`

## Context

A meeting is recorded as two tracks: the microphone (the owner, `self`) and the system
audio (everyone else). Each is transcribed on its own and merged by time (ADR-0017). When
the owner uses speakers instead of headphones, the microphone also hears the other
participants, far more quietly than the owner's own voice. Silero VAD rightly keeps that
audio, because it is speech. Whisper then decodes the faint copy into degraded text, or
into a stock phrase such as 「ご視聴ありがとうございました」, and the merged transcript shows
it as the owner's words next to the clean system-track text of the same moment.

This was found by comparing a private two-track meeting with the meeting tool's own
automatic transcript, using `eval/stt/reference.py`. That script aligns the two locally
and prints only aggregate numbers. The meeting content, names and length stay private;
only counts and levels are recorded here. The reference is another automatic transcript,
so differences are disagreements, not proven errors. Leaked segments are the exception:
the levels below identify them without relying on the reference's wording.

Within the time span both transcripts cover:

| Mic segments | Count | Mic level (median dBFS) | System level at the same time |
| --- | ---: | ---: | ---: |
| Supported by the reference | 115 | −23.7 | −89.7 (silent) |
| Stock hallucination phrase, not in the reference | 6 | −50.1 | −24.7 |
| Other text not in the reference | 29 | −45.9 | −25.9 |

All 35 unsupported mic segments occurred while the system track had speech, and only 5
resembled the system-track text closely enough to be caught as a plain duplicate. They
sit about 25 dB below the owner's voice and below the system track: leaked remote audio,
not the owner speaking.

## Decision

After both tracks are transcribed and before they are merged
(`transcribe_meeting_dual_track_with_options`), `crosstalk::filter_mic_track`:

1. Decodes each track again into a 100 ms energy envelope, one track's PCM at a time.
2. Measures each mic segment on the mic track and on the system track at the same moment.
   Mic time `t` is system time `t + mic_offset_ms`, the convention of `merge_tracks`.
3. Learns the owner's level as the median mic level of mic segments that are at least
   20 dB louder than the system track. With fewer than three such segments, nothing is
   dropped.
4. Drops a mic segment when the system track is active (above −45 dBFS), the mic is
   quieter than the system track, **and** the mic is at least 10 dB below the owner's
   level. A reply spoken over someone else stays, because it is loud on the mic.

It is best effort: if a track cannot be decoded again, the mic transcript is unchanged. It
prints one diagnostic line (own level, dropped / total). Live captions, single-track
recordings and re-running speaker separation (which does not re-transcribe) are unchanged.

### Thresholds

On the same meeting, simulated over the saved transcript:

| Own-voice margin | Mic vs system | Dropped: phrase / other unsupported / supported | CER vs reference |
| --- | --- | --- | ---: |
| (no filter) | | 0 / 0 / 0 | 21.2% |
| 6 dB | below | 6 / 23 / 3 | 18.6% |
| **10 dB** | **below** | **6 / 19 / 1** | **18.9%** |
| 15 dB | below | 6 / 19 / 1 | 18.9% |
| 10 dB | 6 dB below | 6 / 18 / 1 | 18.9% |
| 10 dB | 6 dB above | 6 / 19 / 1 | 18.9% |

The result barely depends on the mic-versus-system margin; the own-voice margin decides it.
10 dB drops every stock phrase and two thirds of the other unsupported text while keeping
114 of 115 supported segments. 6 dB scores slightly better here but drops three supported
segments, so the safer value was chosen.

### Check on the real pipeline

`meeting_cli` runs this Rust filter on the same meeting's saved audio: both tracks, then
the product merge, with no speaker separation. It ran on an Apple Silicon Mac with Metal,
automatic language and greedy decoding. Each row is one Whisper run, scored without and
with the filter:

| Model | Without filter | With filter | Without fillers | Mic segments dropped | Stock phrases |
| --- | ---: | ---: | ---: | ---: | ---: |
| turbo | 21.2% | **19.2%** | 18.7% → 16.6% | 56 | 11 → 0 |
| full large-v3 | 22.9% | **20.5%** | 20.8% → 18.3% | 48 | 10 → 0 |

The last two columns count the whole recording, including the part the reference does
not cover. Character errors fell by 212 (turbo) and 257 (full). The cost shows on turbo:
reference text with no mojiroku text at all rose from 188 to 233 characters, and mic-side
reference cues with no mojiroku segment from 1 to 8. A few quiet real remarks are dropped
with the leak.

The app's own saved transcript of this meeting (full model) scored 21.2% without the
filter, while a fresh full-model run scored 22.9%. Differences of a point or two between
runs and models are within run-to-run variation, so this meeting does not decide turbo
versus full (ADR-0034).

## Alternatives considered

- **A blocklist of stock phrases.** It removes 「ご視聴ありがとうございました」 but not the
  degraded duplicates (29 of 35 segments here), and it could delete a phrase that was
  really said.
- **Acoustic echo cancellation at capture.** It would change the recording itself and the
  capture path on every Mac; much larger than this problem needs.
- **Gating the mic PCM before Whisper.** It changes Whisper's windows and needs a frame
  decision; the measurement was per segment, and a segment decision is simple to test.
- **Lower VAD sensitivity.** The leaked audio is speech, so VAD is right to keep it.

## Consequences

- A quiet remark made while someone else talks, more than 10 dB below the owner's usual
  level, is dropped as well. Owners with headphones are unaffected in practice: without
  leakage, the mic is rarely quieter than the system track during speech.
- Each two-track transcription decodes both tracks once more. That step was not timed on
  its own. Peak memory is one track's 16 kHz PCM, and no model is loaded at that point.
- `eval/stt/reference.py` and the `meeting_cli` example can repeat this measurement on any
  private meeting with an exported transcript, printing numbers only.
