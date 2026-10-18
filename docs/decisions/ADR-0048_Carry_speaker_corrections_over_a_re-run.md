# ADR-0048: Carry speaker corrections over a re-run

- Date: 2026-10-07
- Status: Accepted
- Related: Issue #19, Issue #102, ADR-0024 (re-run with name carry-over), `docs/spec.md` §9.1

## Context

A user can change the speaker of a single line (`set_segment_speaker`, Issue #19). Until
v0.7.0 those corrections were safe only because the detail view hid speaker separation for
recordings that already had speakers. Issue #102 added 「話者分離をやり直す」 for exactly
those recordings, and the re-run rewrites every line's speaker
(`replace_speaker_assignments` deletes and re-inserts the segments). Every correction was
lost without a warning, and nothing recorded which lines had been corrected, so the loss
could not even be detected.

A correction is a stronger statement than a rename: it says the separation was wrong for
that line. Dropping it silently is not acceptable (§9.1).

## Decision

1. **Corrections are stored** in `speaker_corrections` (schema v8): one row per corrected
   line with the speaker the separation predicted and the speaker the user chose. A second
   correction of the same line keeps the first prediction. Choosing the predicted speaker
   again deletes the row (an undo). Re-transcribing deletes all rows, since `idx` may then
   point at different text.
2. **A re-run carries corrections over** (`correction::carry_corrections`). Line indexes do
   not change, because the text is not re-transcribed. Only the speaker ids change, so each
   chosen speaker X is mapped to a new id:
   1. the new speaker that most of X's **uncorrected** lines received (corrected lines do not
      vote; ties go to the smaller id),
   2. otherwise the voiceprint match used for name carry-over (`match_speaker_ids`, same
      threshold),
   3. the meeting's own speaker (`self`) maps to itself, as the mic track is never
      re-separated.
3. **A correction that cannot be mapped becomes unassigned ("?")**, and the job reports the
   count. This includes a correction whose chosen speaker and originally predicted speaker
   map to the same new speaker: the re-run merged the two people the user told apart, so
   keeping the line on that speaker would silently undo the correction. The UI shows it as a notice after the re-run. The line is not left on the new
   prediction, which the user had already rejected once. A line confirmed by hand (rule 4)
   has no rejected speaker, so this case does not apply to it.
4. The stored rows are rewritten in the new ids. `predicted` becomes the re-run's
   assignment, so the table always compares the current separation with the user's choice.
   A row whose new prediction matches the user's choice is kept as a line confirmed by hand.
   Only the user choosing the predicted speaker again deletes a row. For a confirmed row
   that is the line's current speaker, so choosing it again deletes the row without changing
   the line or marking summaries stale.

## Evidence

The development build was run on a copy of a real database (a separate `HOME`, so the
installed app and its MCP servers were untouched). Five corrections were written to two
recordings, then speaker separation was re-run through the job queue:

| Correction | After the re-run |
|---|---|
| File: a line moved to the main speaker | Kept, on that speaker's new id |
| File: a line cleared to unknown | Still unknown |
| File: a line moved to a short speaker that the re-run merged into the original one | Unassigned, counted (rule 3) |
| Meeting: a guest line moved to `self` | Still `self` |
| Meeting: the `self` line moved to a guest | Kept, on that guest's new id |

The first run, before the merge rule in 3 existed, put the third line back on the speaker
the user had rejected. The bundled MCP server built from the same commit returned the
corrected speakers in `get_meeting`. Meeting content stays private; only these outcomes
are recorded here.

## Consequences

- The table gives increment 2 of Issue #19 (corrections as evaluation labels) its data.
  Its rows are biased toward errors the user noticed (§9.1).
- Corrections made before v8 were never recorded and cannot be carried over.
- The unmapped notice is only shown while the app is open. Afterwards the "?" chips are
  the only sign.
- Any new path that rewrites `segments` must decide what happens to `speaker_corrections`.
