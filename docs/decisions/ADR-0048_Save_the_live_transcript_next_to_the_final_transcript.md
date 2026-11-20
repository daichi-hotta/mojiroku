# ADR-0048: Save the live transcript next to the final transcript

- Date: 2026-10-29
- Status: Accepted
- Related: Issue #154, Issue #87
- Updates: The "disposable preview" lifecycle of live transcription in [ADR-0017](ADR-0017_会議モードのシステム音声キャプチャ.md). Builds on [ADR-0038](ADR-0038_Background_model_downloads_and_saved_live_translations.md).

## Context

A meeting produces two transcripts. The live transcript is made during recording from
mixed mic and system audio with turbo, about every 3.5 s, without speakers. The final
transcript is made after Stop from the saved per-track audio with the selected model and
speaker attribution. Only the final transcript was saved. The live worker kept the last
80 committed lines for display, and Stop dropped them. The only live text that survived
was the source caption of a completed translation (ADR-0038), so nothing was kept when
translation was off.

The two transcripts can differ. In #87 the live captions showed speech that the final
transcript lost, and there was no saved live transcript to compare against. Users also
want to read again what they saw during the meeting.

## Decision

### The live worker keeps the whole meeting

`live_stt` keeps a `LiveCaptionArchive` (in `mojiroku-core`) for each session next to the
80-line display window. Every committed line goes into the archive under the same caption
ID that `meeting://live` sends, so saved translations (`live_translations.source_id`) match
their caption. Each emit also records the unfinished tail on screen. The archive is shared
with the worker through `Arc<Mutex<_>>`, so lines committed before a worker panic are kept.

`stop_and_take` stops and joins the worker, then returns every committed line followed by
the last tail (marked `committed = false`). Discard and the other teardown paths call
`stop`, which drops the archive.

### Limits

The archive enforces the stored limits while the meeting runs: at most 20,000 rows, 16 MiB
of text, and 4,096 bytes per caption (longer text is cut at a character boundary; Whisper
segments are far shorter). After the limit, later lines are not archived, but live display
and recording continue. At about one line per 3.5 s, 20,000 rows is about 19 hours.

### Storage

SQLite schema v8 adds `live_captions`: recording ID, caption ID, text, and the committed
flag, with `ON DELETE CASCADE`. `insert_recording_with_live_history` inserts the recording,
its search row, its live translations, and its live captions in one transaction.
`stop_meeting_recording` validates the captions again; if they still fail validation, it
logs and saves the meeting without them rather than losing the recording.

`replace_transcript` and speaker re-runs touch only the final transcript. The live
transcript does not change after Stop.

### Display

The detail view of a meeting has a read-only "Live transcript" tab between the transcript
and the saved translations. It says that the text is what was shown during the meeting,
has no speakers, can have gaps, and may differ from the final transcript. The unfinished
line from Stop is marked. Recordings without a live transcript show an empty state.

The final transcript stays the only source for summaries, search, export, titles, and MCP.

## Alternatives

| Alternative | Reason not selected |
|---|---|
| Collect the captions in the frontend and send them at Stop | Event snapshots carry only the 80-line window, and the frontend would need its own ID bookkeeping. The worker already owns the lines. |
| Write each caption to SQLite while recording | Continuous writes and a crash-recovery protocol beyond saving a finished meeting (same reason as ADR-0038). |
| Store the live transcript as a second set of `segments` | `segments` carries timing and speakers that live lines do not have, and every reader of `segments` would need to tell them apart. |
| Run a final live pass over the remaining audio at Stop | Adds Whisper work to Stop and delays the final transcript. The last tail already shows what the user saw. |

## Consequences and verification

Meetings now store their live text, which is small next to the audio (a few hundred KB for
a long meeting). Only captions taken at Stop are saved; quitting the app before Stop still
loses them, as for translations. Recordings made before v8 have no live transcript. Live
lines have no timestamps yet, so they cannot jump to audio playback.

The read-only MCP binary rejects a database newer than the schema it was built with, so it
must be rebuilt with the v8 core.

Core tests cover the archive (all lines kept beyond the display window, last tail replaced,
limits, character-boundary cuts), validation, transcript replacement, delete cascade,
rollback of the whole save, and migration from v7 with reopen. `live_stt` tests check that
every line the UI saw is saved under its ID and that stopping without a session saves
nothing. A frontend test covers the tab content.
