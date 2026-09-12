# autoTrim

A small always-on monitor for developer machines that notices when memory is
being wasted, tells you what is holding it, and can safely reclaim it.

## The problem

Modern dev machines fill up with things that hold memory while doing nothing:
dozens of idle AI agent sessions (Claude Code, Codex, Cursor, and friends),
a Chrome with ninety live tabs across five profiles, Electron apps left open
for days, and a machine that has not been restarted in two months. Activity
Monitor shows a flat list of helper processes. Chrome's task manager only sees
Chrome. Neither remembers what happened an hour ago, neither tells you what to
do, and neither notices while you are busy.

autoTrim runs in the background, samples continuously, groups memory by the
thing you would actually close, applies a small set of rules, and notifies you
with a number and an action attached.

## Scope

In scope:

- **Observe.** Sample system memory, swap, compressed memory, uptime, and every
  process, cheaply and continuously. Group processes by app, with helpers
  rolled up under their parent. Keep a rolling history.
- **Attribute.** Treat AI agent sessions as a first-class group: which agent,
  which host (desktop app, editor, terminal), which project, how old, how idle,
  how much memory. Treat browsers as a first-class group: renderer count,
  profiles, memory.
- **Advise.** A deterministic rule set that turns observations into a short
  list of actions with evidence and expected recovery. Examples: restart after
  long uptime with swap full, close stale agent sessions, tighten Chrome's
  Memory Saver, quit an idle heavyweight app, name the app that grew fastest in
  the last hour.
- **Notify.** Native notifications, rate limited per rule, with quiet hours.
  Reminders are the product.
- **Reclaim.** A small set of narrow, logged, reversible actions: close an idle
  agent session (its transcript stays on disk and the resume command is logged),
  discard browser tabs, quit an app. An opt-in auto mode runs these on a fixed
  policy with a notify-first grace period.
- **Expose.** A local interface (MCP over localhost) so any agent can read the
  snapshot and history and call the same safe actions. A `scan` command that
  prints a shareable one-shot report.
- **Run everywhere.** macOS first, then Windows and Linux. One binary, three
  modes: `scan`, `daemon`, `tray`.

Out of scope, deliberately:

- **Orchestrating agents.** No starting sessions, no sending prompts, no plans,
  no queues, no routing work between agents. autoTrim only ever stops things
  or suggests stopping them.
- **Being an agent.** No model calls inside the daemon. Every decision the
  daemon makes is a rule you can read in the source.
- **Memory "cleaning."** No purge tricks, no cache clearing, no claims that
  freeing inactive memory speeds anything up.
- **General system monitoring.** Not a replacement for Stats or iStat Menus.
  No CPU graphs, fans, network, or disk dashboards beyond what the memory
  rules need.
- **Arbitrary process killing.** Actions are named verbs on recognized targets,
  never a generic kill.

## Principles

- **Tiny footprint.** The daemon stays under 20 MB resident and negligible CPU,
  or it undermines its own pitch. This is a public budget.
- **Deterministic.** Same snapshot, same advice. Thresholds live in one config
  file with sane defaults.
- **Reversible and logged.** Every action writes what it did and how to undo
  it. Closing a session logs the resume command.
- **Never surprise.** Auto mode is off by default, notify-first for a grace
  period, and never touches the most recent session per project, a session the
  user is sitting in, or anything on an allowlist.
- **Self-aware.** The daemon and any agent session it runs under are excluded
  from its own actions.
- **Trustworthy by inspection.** Open source, normal user permissions, one
  command to uninstall.

## Version one

1. `autotrim scan`: one-shot report. System totals, top app groups, agent
   sessions with idle state, browser breakdown, advice cards. Text and JSON.
2. `autotrim daemon`: continuous sampling, rolling history on disk, rule
   evaluation, notifications. Installable as a launchd agent.
3. `autotrim tray`: menu bar item showing pressure and the current advice, with
   one-click actions that talk to the daemon.
4. Actions: close idle agent session, with logging and resume command.
   Auto mode behind a flag.
5. MCP server inside the daemon exposing snapshot, history, sessions, and
   actions.

Later: Windows and Linux ports, Chrome per-tab attribution (needs a feasibility
spike; stable Chrome does not expose renderer-to-tab mapping), localhost
dashboard, Tauri window.

## Architecture

```
src/
  main.rs      CLI entry: scan | daemon | tray
  system.rs    memory totals, swap, compressed, uptime (platform-specific)
  procs.rs     process snapshot: pid, parent, exe, args, cwd, rss, cpu, age
  groups.rs    roll processes up into app groups
  agents.rs    detect agent sessions, host, project, idle state
  browser.rs   Chrome/Chromium breakdown
  rules.rs     deterministic advice
  report.rs    text rendering
```

Everything upstream of `rules.rs` produces one `Snapshot` struct, serialized
to JSON. The daemon, the tray, the MCP server, and the `scan` command all
consume that struct. Platform-specific code stays in `system.rs` and behind
`cfg` blocks in `procs.rs`.

## Status

Early. Three commands work on macOS:

- `autotrim scan`: one-shot report. System totals, top holders, browser
  breakdown, agent sessions, and the first four rules (restart, stale
  sessions, browser sprawl, heavy app). Text and `--json`. Release binary is
  about 1.3 MB and a scan peaks around 10 MB resident.
- `autotrim daemon`: samples every 30 s with one long-lived system handle, so
  each CPU reading is a 30 s average rather than an instant. Keeps a rolling
  window (default 10 min) per session, keyed by pid and start time, and
  tracks how long each has been continuously quiet. A session is only called
  stale after it has been observed quiet for a minimum period (default 15 min),
  however old it is, so a freshly started daemon never judges anything in its
  first minutes. Writes `latest.json`, one compact history line per tick into
  daily `history-YYYY-MM-DD.jsonl` files (7 days kept), and its window state,
  under the platform data directory. Prints advice as it appears and resolves.
  Runs in the foreground for now; `--once` takes a single tick and exits.
- `autotrim status`: renders the daemon's latest snapshot without sampling.

Data lives in `~/Library/Application Support/autotrim` on macOS,
`$XDG_DATA_HOME/autotrim` on Linux, `%LOCALAPPDATA%\autotrim` on Windows.
Override with `AUTOTRIM_DATA_DIR`.

Known gaps, in the order they should be fixed:

- **No notifications yet.** The daemon logs advice transitions to stdout.
  That is the hook where native notifications plug in.
- **Not installable as a service yet.** No launchd plist, no Task Scheduler
  entry, no systemd unit.
- **Quiet means low CPU.** For Claude Code, the transcript's last real user
  or assistant entry would be a better signal, but mapping a process to its
  transcript needs a spike (the process does not carry its session id).
- **`vm_stat` and `memory_pressure` are shelled out** on every tick. Fine for
  a scan, wasteful for a daemon. Replace with mach calls.
- **Resident size, not footprint.** Activity Monitor shows physical footprint,
  which counts compressed pages. Numbers here run a little lower than it.
- **Codex under the ChatGPT app is reported as a session** with `/` as its
  project. It is really a long-lived backend. Worth a distinct label.
- **macOS only.** Windows and Linux compile but report no compressed memory,
  no free percentage, and no browser profiles.

Build and run:

```bash
cargo build --release && ./target/release/autotrim scan
```

```bash
./target/release/autotrim daemon --interval 30
```
