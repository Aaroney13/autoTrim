# Privacy and local data

autoTrim inspects local processes, supported agent session files, and supported
Chromium browser profiles to attribute memory use and identify idle work. That
data can contain confidential project names, prompt excerpts, URLs with tokens,
and account details. Treat the data directory and exported reports as sensitive.

## What is read

- Process names, executable paths, command lines, working directories, process
  relationships, resource use, and listening ports identify memory holders.
- Agent metadata and transcripts identify sessions and their last activity.
  Readers may scan transcript records, including message contents, to extract
  timestamps, titles, and a first-user-prompt excerpt (shortened to 80 characters).
  autoTrim does not save a copy of the full conversation. Supported sources
  include Claude Code session/transcript files, Codex rollouts, Copilot events,
  and Cursor chat stores. Access is limited to what the current OS user can read.
- Browser session files expose open tabs' full URLs, titles, profile, pinned and
  active state, and activity times. Profile metadata can include a display name
  and email address. autoTrim does not request page contents from those URLs;
  browser session files can contain navigation history while being parsed.

There is currently no setting to redact URLs or prompt-derived names, or to
disable just the agent or browser reader. If this access is unsuitable, stop
the daemon and quit the app; one-shot scans also read these sources.

## What is saved and for how long

| File | Contents | Retention |
| --- | --- | --- |
| `latest.json` | Latest complete snapshot: sessions, prompt excerpts/titles, project and transcript paths, tabs with full URLs, browser profiles, resource use, ports, advice, and auto-mode status | Replaced on each successful tick |
| `state.json` | Rolling observations, session identifiers, notification/pending-action state, port labels, and trend names that may derive from prompts or projects | Replaced on each successful tick; rolling series expire in memory |
| `history-YYYY-MM-DD.jsonl` | Resource measurements, app names, session identifiers, project names, and advice identifiers; no full tab list or conversation | Files older than the UTC date cutoff are removed on daemon ticks; `retention_days = 7` by default, so the cutoff date and today are retained |
| `actions.jsonl` and `.1`–`.3` | Action intent/outcome and recovery instructions; may include session names, paths, IDs, full tab URLs, and target/process identities | Current log plus three backups, each at most 5 MiB for autoTrim writes; oldest records are discarded on rotation |
| `daemon.log` and `.1`–`.3` | Advice changes, auto-mode targets and recovery commands, startup/configuration messages, storage failures and recovery | Same 5 MiB / three-backup limit; launchd also sends stdout/stderr here |
| `crash.json` | Most recent fatal error or Rust panic, time, version, PID; panics include source location and stack trace, but omit the panic payload | Replaced by the next report; no automatic age expiry |
| `config.toml`, `daemon.pid` | Settings and current daemon PID | Settings remain until removed; PID file is removed on normal exit/unwind |

Fatal error messages and stack traces can include local paths and error context.
OS-level termination (for example, SIGKILL, an out-of-memory kill, or power loss)
cannot run the Rust panic hook and may leave no crash report. Reports are best
effort when the filesystem is full or unavailable. A failed tick does not erase
the previous snapshot: saved views can be stale until writes recover, and missed
history samples are not backfilled.

Rotation bounds space, not age: a quiet action log can retain old records for a
long time. Retention cleanup runs only while the daemon runs. Old, oversized
logs are reduced to a bounded tail when next written. Native notification
history, backups, and files you export have their own retention policies.
Log views also read at most the newest 5 MiB of each retained file, including
oversized logs from older versions.

## Where data lives and who can read it

- macOS: `~/Library/Application Support/autotrim`
- Linux: `$XDG_DATA_HOME/autotrim`, or `~/.local/share/autotrim` when unset
- Windows: `%LOCALAPPDATA%\autotrim`
- `AUTOTRIM_DATA_DIR` overrides these locations. Use a dedicated directory:
  autoTrim restricts the directory's permissions, including an existing one.

On Unix, autoTrim creates the directory with mode `0700` and its files with
mode `0600`, including atomic temporary files, independently of a permissive
umask. The daemon and service installer tighten recognized files left by older
versions. Log writes reject symbolic links and multiple hard links. New macOS
launch agents also specify umask `077`. Reinstall the login service with
`autotrim service install` after upgrading to refresh its launchd settings.

On Windows, files inherit the data directory's ACL; this version does not set a
custom per-user ACL. Check it when choosing a custom or shared directory.
Permissions do not protect against other software running as you or an
administrator. Files are plain JSON/text, with no application-level encryption.
OS disk encryption and backup access controls remain your responsibility.

## Network, notifications, and sharing

The monitoring daemon makes no model calls and uploads no snapshots, action
logs, or crash reports. There is no telemetry or hosted crash-reporting service.
Optional port identification (`probe_ports = true` by default) sends a fixed
HTTP `GET /` to a listening service on this machine: loopback for wildcard
listeners, otherwise the listener's bound address. It sends no prompt, tab URL,
or credentials, follows no redirects, and reads a bounded response to identify
the service. Disable it with `autotrim config set probe_ports=false`.

The menu bar app's update checks and downloads contact its configured release
host. Those requests are separate from monitoring and do not attach monitoring
files. The release host can observe normal connection/request metadata.

Native notifications can display session/project names and advice on screen or
in the OS notification history. `autotrim config set notify=false` disables
daemon notifications; `autotrim daemon --no-notify` overrides them for that run.
Storage failures notify immediately and at most hourly while they continue,
with a recovery notification when writes resume. Repeated crash notifications
are suppressed when a recent local crash report is available.

The app and CLI read the same local files. The planned localhost agent interface
is not implemented. Nothing is automatically sent to a support team. Inspect
and redact reports, logs, screenshots, and recovery commands before sharing
them; copying a report or redirecting CLI output creates a separate copy outside
autoTrim's file-permission and retention controls.

## Removing data

Run `autotrim service uninstall`, quit the menu bar app, and stop any manually
started daemon. Then delete the autoTrim data directory (or your override),
including log backups and any `.tmp` files left after an abrupt stop. Uninstalling
the service/app intentionally keeps this directory. Deleting it removes
autoTrim's history, settings, and recovery instructions; it does not delete
the original agent transcripts or browser data. Remove exported copies,
notification history, and backup copies separately where needed.
