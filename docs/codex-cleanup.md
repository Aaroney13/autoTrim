# Scheduled Codex cleanup

Codex desktop tasks share an app-server process. AutoTrim's ordinary automatic
session closer deliberately excludes that process: killing it affects every
task. The desktop build inspected on September 14, 2026 uses a private stdio
connection, so the standalone AutoTrim daemon has no supported connection on
which to send task lifecycle requests.

The separate Codex heartbeat named **AutoTrim Codex cleanup** is **paused**.
The user wants cleanup implemented as an AutoTrim feature; the heartbeat was
created in error as a substitute. No real tasks were archived by it. The policy
below records that experiment, not an implemented native cleanup feature.

Native integration would let AutoTrim apply deterministic rules and send Codex
lifecycle commands without model calls. It requires a supported control
connection to the backend that actually owns the loaded tasks. Starting a
second app-server does not provide that connection to the desktop backend.
AutoTrim's current Off/Preview/On controls do not control the separate heartbeat.

## Policy

- Consider only local Codex tasks currently observed loaded by AutoTrim and
  confirmed idle by the Codex app. Saved, already unloaded history is excluded.
- Require at least six hours since the latest task activity. Retain the newest
  task in each project, including ties.
- Protect the cleanup task itself, pinned tasks, tasks attached to any automation
  (including paused automations), exclusions in AutoTrim's configuration, and
  tasks with unfinished turns, approvals, queued work, or ongoing goals.
- Inspect descendants because Codex archive can cascade. A running, protected,
  or uncertain descendant protects its parent. Do not target helpers separately.
- Record a candidate on one check; require another check at least ten minutes
  later with unchanged activity before archiving. Activity or loss of evidence
  cancels the candidate. With hourly checks, the usual grace is one hour.
- Recheck the task and protections immediately before each archive. Save its
  exact ID, title, transcript path, and restoration instructions before acting.
  Archive at most two tasks per run. Never delete tasks, terminate their backend,
  cancel scheduled work, or remove worktrees as part of this cleanup.
- An error or missing evidence means skip, not permission to broaden cleanup.

Pending candidates live in `codex-cleanup-state.json` and an append-only recovery
journal in `codex-cleanup-actions.jsonl`, in AutoTrim's data directory. These are
separate from the native Actions view. An unfinished intent is reconciled before
retrying. Restoration uses `set_thread_archived` with the recorded task ID and
`archived: false`; resuming the CLI is not a substitute for unarchiving in the app.

## Verification and limits

An isolated test against the installed Codex Desktop 0.154.0-alpha.6.2 backend
resumed a synthetic persisted task, confirmed it in `thread/loaded/list`, archived
it, confirmed its removal from that list, and unarchived it successfully. The test
used a temporary Codex home and made no model request or changes to real tasks.

After real cleanup, compare a fresh AutoTrim scan for the archived task's open
transcript. Report archive success separately from evidence that it unloaded.
RAM remains a shared process measurement: neither this test nor the protocol
provides per-task memory or guarantees a particular reduction in process RAM.
The heartbeat reports meaningful cleanup or failures and stays quiet when there
is no actionable change.
