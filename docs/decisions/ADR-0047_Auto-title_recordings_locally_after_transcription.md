# ADR-0047: Auto-title recordings locally after transcription

- Date: 2026-09-25
- Status: Accepted
- Related: Issue #4, ADR-0007 (summary sidecar), ADR-0021 (heavy-job serialisation), ADR-0024 (job queue)

## Context

Microphone recordings and meetings without a calendar event are saved as 「録音」/「会議」
(`Recording`/`Meeting`). A history list full of identical names is hard to scan. The
prompt and output clean-up for titles (`summarize/title.rs`) were tuned against 19 real
meetings earlier in Issue #4, but nothing called them.

## Decision

1. **Automatic titles run only on the local summary model, and only when it is already
   on disk.** The transcript is never sent to a cloud provider without an explicit action,
   even when the summary engine is set to cloud, and no multi-gigabyte download starts on
   its own (`models::cached_summary_model`).
2. **Only default names are replaced.** The job runs for microphone recordings and
   meetings whose title is one of the default names, with at least 5 segments and 80
   characters of text (`should_auto_title`). File imports keep their file names, and
   calendar or hand-typed titles are never touched. The title is checked again right
   before saving, so a rename made while the title was being generated wins.
3. **It is a background job (`kind = "title"`)** queued after a successful
   transcription, so it shares the single heavy-job slot with transcription and speaker
   separation. It is excluded from the recording's visible state: the history row, the
   detail view's in-progress panel, the job list and toasts ignore it. A failure leaves
   the default name and is only logged.
4. **The detail view has an explicit 「タイトルを生成」 button.** Because the user asked
   for it, it follows the summary engine setting, including cloud (with a confirmation
   before sending). The local path uses the cached model and reports
   `error.title.model_missing` rather than downloading one.

The sidecar is called with a 48-token budget, the value the prompt was tuned with.

## Evidence

Local run on 14 real meetings with Qwen3.5-4B (the small tier, ADR-0044), 48 tokens,
`--no-think`: all 14 outputs were usable Japanese titles, with no simplified Chinese and
no dates, in 4–58 s depending on transcript length. The meeting content stays private;
only the counts are recorded here.

## Consequences

- On machines with a summary model, every mic recording or meeting gets one extra
  sidecar run after transcription (the same memory profile as a local summary,
  serialised by the heavy-job permit, so no new crash surface). It can delay the next
  queued job by the generation time.
- New code that reads a recording's job state must keep excluding `TITLE_JOB_KIND`.
