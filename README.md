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
  main.rs         CLI entry: scan | daemon | status | watch | log | service
  system.rs       memory totals, swap, compressed, wired, cpu, load, uptime
  procs.rs        process snapshot: pid, parent, exe, args, cwd, rss, cpu, age
  groups.rs       roll processes up into app groups
  agents.rs       detect agent sessions: host, project, idle evidence
  transcripts.rs  what agents leave on disk: Claude Code session files and
                  transcripts, Codex rollouts
  openfiles.rs    a process's open files (libproc on macOS, /proc on Linux)
  browser.rs      Chrome/Chromium breakdown
  ports.rs        listening ports and their owners
  rules.rs        deterministic advice and the session-state decision
  daemon.rs       sampling loop, rolling windows, history, notifications
  notify.rs       native notification delivery
  service.rs      launchd install/uninstall/restart/status
  watch.rs        live terminal view and log printing
  paths.rs        data directory per platform
  report.rs       text rendering
  fmt.rs          bytes, durations, dates
```

Everything upstream of `rules.rs` produces one `Snapshot` struct, serialized
to JSON. The daemon, the tray, the MCP server, and the `scan` command all
consume that struct. Platform-specific code stays in `system.rs` and behind
`cfg` blocks in `procs.rs`.

## Status

Early, but the loop is closed on macOS: observe, judge, notify, install.

- `autotrim scan`: one-shot report. System totals, top holders, browser
  breakdown, agent sessions, and the first four rules (restart, stale
  sessions, browser sprawl, heavy app). Text and `--json`. Release binary is
  under 2 MB and a scan peaks around 10 MB resident.
- `autotrim daemon`: samples every 30 s with one long-lived system handle, so
  each CPU reading is a 30 s average rather than an instant. Keeps a rolling
  window (default 10 min) per session, keyed by pid and start time, and
  tracks how long each has been continuously quiet. Writes `latest.json`, one
  compact history line per tick into daily `history-YYYY-MM-DD.jsonl` files
  (7 days kept), and its window state. Sends a native notification when
  advice first appears and again every 4 hours while it persists
  (`--remind-every-hours`, `--no-notify`); the send log is persisted so a
  restart does not repeat itself. `--once` takes a single tick and exits.
- `autotrim status`: renders the daemon's latest snapshot without sampling.
- `autotrim watch`: a live terminal view that redraws every few seconds from
  the daemon's latest snapshot (or its own scan when no daemon is running),
  with the tail of the daemon log underneath. The quickest way to see what
  it is doing.
- `autotrim log [-n 50] [-f]`: print or follow the daemon log.
- `autotrim service install | uninstall | restart | status`: launchd agent on
  macOS that starts the daemon now and at every login, logging to
  `daemon.log` in the data directory.

- `autotrim config`: print the effective settings and where they came from.
  `autotrim config init` writes `config.toml` in the data directory with
  every setting, its default, and a comment. Flags override the file, the
  file overrides the defaults. Thresholds, intervals, notification cadence,
  and ignore lists for ports, apps, and projects all live there.

Every report also carries whole-machine CPU and load average, CPU per app
group and per session, and a table of listening TCP/UDP ports with the app
group or agent session that owns each one and how long it has been open.
The daemon tracks port age across ticks (a port cannot predate its process,
so first sight uses the owner's start time), and a fifth rule reports old
local servers: a listener that is not an app, not a system process, and not
part of an agent session, quiet, and open longer than a day by default. The
verb that stops one is part of the reclaim work still to come.

How "idle" is decided, strongest evidence first:

1. **Busy CPU right now means active**, whatever else is known. The daemon's
   window mean is trusted at 2%; a one-shot sample only overrides transcript
   evidence when it is unmistakably busy, because a single 1.5 s sample on a
   swapping machine jitters by a few percent.
2. **Claude Code publishes enough to know the truth.** Every running process
   has `~/.claude/sessions/<pid>.json` with its session id and working
   directory, and the transcript sits at a predictable path under
   `~/.claude/projects`. autoTrim reads the tail of that transcript for the
   last real user or assistant entry (host-app bookkeeping entries are
   ignored, which is why the file's modification time is not a signal) and
   reports idle time from it. A session idle longer than the stale threshold
   (default 6 h) is stale, immediately, even on a one-shot scan.
3. **Codex publishes it a different way.** Codex keeps one rollout file per
   thread under `~/.codex/sessions` and holds the live ones open, so a Codex
   server's own open-file table (read with libproc on macOS, `/proc` on
   Linux, no `lsof`) says which threads it is hosting. Idle time is the
   newest response or event across those files, and the project is the
   working directory in the newest thread's metadata rather than the
   server's own `/`. A Codex server with no thread open falls through to
   the next rule.
4. **Other agents fall back to the daemon's quiet window.** A session must be
   observed quiet for a minimum period (default 15 min) before it is called
   stale, however old it is, so a fresh daemon never judges anything in its
   first minutes.
5. **A one-shot scan of an agent with no transcript** falls back to age plus
   a quiet sample, which is the weakest signal here and is labelled as such
   in the JSON (no `idle_secs`, no `quiet_for_secs`).

Memory counters on macOS come from `host_statistics64` and the
`kern.memorystatus_level` sysctl, the same sources `vm_stat` and
`memory_pressure` print, with no subprocesses.

Data lives in `~/Library/Application Support/autotrim` on macOS,
`$XDG_DATA_HOME/autotrim` on Linux, `%LOCALAPPDATA%\autotrim` on Windows.
Override with `AUTOTRIM_DATA_DIR`.

Known gaps, in the order they should be fixed:

- **Transcript tails are re-read every tick.** A Codex server with a dozen
  threads open costs a few megabytes of reads per tick. Cache by file length.
- **Notifications carry no buttons.** A bare binary cannot register
  actionable notifications on macOS; that needs an app bundle, which comes
  with the tray.
- **No reclaim actions yet.** The advice tells you what to close; the close
  verbs (agent session with its resume command logged, old local server,
  browser tab discard) are the next feature, and auto mode sits behind them.
- **No quiet hours** for notifications yet; the config file is where they
  will go.
- **macOS only** for the service, the memory counters beyond swap, and
  browser profiles. Windows and Linux compile and run `scan` and `daemon`.

## Getting started

Build once, then pick how you want to run it.

```bash
cargo build --release && ./target/release/autotrim scan
```

Always on, starting now and at every login (macOS):

```bash
./target/release/autotrim service install
```

Or in the foreground, in a terminal you keep open:

```bash
./target/release/autotrim daemon
```

Then, in another terminal, watch it work:

```bash
./target/release/autotrim watch
```

`autotrim status` prints the latest snapshot once, `autotrim log -f` follows
the daemon log, and `autotrim service uninstall` removes the login service.
Rebuilt the binary? `autotrim service restart` picks up the new one.

## Decisions

Things that came up and where they landed.

- **Emergencies stay deterministic.** When memory is critical the wrong move
  is to start a model that needs memory to think, and a wedged machine cannot
  run one anyway. The planned emergency tier notifies loudly and, in auto
  mode, runs the reclaim verbs in a fixed order. No model in that loop.
- **Hand-off to your agent is a launch, not a decision.** A planned
  `autotrim fix` starts your own agent (Claude Code or Codex) with the
  current snapshot and the safe action verbs, so it can diagnose and act with
  full context. It runs when you ask for it. An opt-in flag may later fire it
  after an emergency has been handled, off by default. The daemon itself
  never calls a model.
- **UI comes as a Tauri tray app** once the daemon and its actions are
  stable: a menu bar item with pressure and the current advice, a window with
  the sessions, ports, and history, one-click actions that talk to the
  daemon over a local socket. The terminal `watch` view is the interface
  until then, and stays afterwards for people who live in a terminal.

