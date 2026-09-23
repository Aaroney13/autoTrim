# Scheduled Codex cleanup

Codex desktop tasks share an app-server process. AutoTrim archives individual
tasks through an already-running optional desktop bridge; it never kills their
shared backend. Starting a second app-server cannot control the desktop backend.

## Native automatic archiving

Enable **Archive idle Codex tasks** in Settings (`auto_archive_codex = true`).
It shares AutoTrim's **Off**, **Preview only**, and **On** controls. The setting
is disabled by default. It uses the session inactivity threshold
(`stale_after_hours`, default six hours) and warning period
(`auto_grace_minutes`, default ten minutes).

- Consider only verified loaded idle tasks, using their latest task update,
  recency timestamp, or transcript modification. Keep the newest task in each
  project, including ties.
- Protect active or unknown tasks, pinned tasks, automation-linked tasks
  (including paused automations), exclusions, unfinished turns/tools, queued
  work, and unfinished goals. Parents with recorded children and helpers remain
  protected to prevent archive cascades.
- Require an unchanged candidate through the warning period and a later scan.
  Activity, protection, or settings changes cancel the warning. Switching from
  Preview to On requires a fresh warning.
- Recheck the task, saved settings, and newest-task protection immediately before
  archiving. Save identity and recovery in native Actions before mutation.
  Archive at most two tasks per pass; never delete transcripts or worktrees.
- Missing evidence means skip. A failed attempt requires a fresh warning before
  qualifying again. Preview records what would happen without archiving.

Pending candidates use AutoTrim's existing persisted policy state. Actions offers
**Restore task**, using the saved identity to unarchive through the owning backend.
Manual **Archive task** remains available for eligible tasks independently of
whether the automatic timer is enabled.

See [implementation details](design.md#individual-codex-task-archiving).

## Verification and limits

Isolated tests against the installed Codex backend use temporary Codex homes and
synthetic persisted tasks. They verify timed-action guards, archive/unload,
restoration, and that the other task and shared backend remain alive. No model
requests or real task mutations are made by these tests.

The backend has no atomic archive-if-idle operation; final checks narrow but
cannot eliminate concurrent activity races. Archive success and verified unload
are tracked separately. RAM remains a shared process measurement, so AutoTrim
cannot attribute memory savings to one task.

## Earlier heartbeat experiment

The separate **AutoTrim Codex cleanup** heartbeat remains **paused**. It was
created in error as a substitute for this native feature and archived no real
tasks. Native Settings does not control that heartbeat, and the native policy
uses neither its proposed separate journal nor its hourly schedule.
