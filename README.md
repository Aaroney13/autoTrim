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
  profiles, memory, every open tab by site and age, and which of those
  pages are conversation UIs or local apps.
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

Later: Windows and Linux ports, localhost dashboard. (Chrome per-tab
attribution had its feasibility spike: tabs, titles, and last-viewed times
come from the browser's own session files, and a tab can be closed by its
session id. Per-tab memory is still not something stable Chrome exposes.)

## Architecture

```
src/
  main.rs         CLI entry: scan | daemon | status | watch | log | tabs |
                  close | stop | close-tab | quit | actions | config |
                  service | open
  system.rs       memory totals, swap, compressed, wired, cpu, load, uptime
  procs.rs        process snapshot: pid, parent, exe, args, cwd, rss, cpu, age
  groups.rs       roll processes up into app groups; what counts as an app per platform
  agents.rs       detect agent sessions: host, project, name, idle evidence,
                  and which processes are an app's own agent engine
  transcripts.rs  what agents leave on disk: Claude Code session files and
                  transcripts, Codex rollouts, Copilot CLI event logs,
                  Cursor chat stores; last activity and session names
  openfiles.rs    a process's open files (libproc on macOS, /proc on Linux)
  browser.rs      Chrome/Chromium breakdown: renderers, profile directories
                  per platform, open tabs, sites, stale tabs
  snss.rs         reader for Chromium session files (windows, tabs, titles,
                  last-viewed times)
  automation.rs   asking another app to do something: close a tab, quit
                  (Apple Events on macOS)
  actions.rs      the reclaim verbs and the action log
  ports.rs        listening ports and their owners
  portlabel.rs    what a port is: known table, command line, HTTP fingerprint
  rules.rs        deterministic advice and the session-state decision
  daemon.rs       sampling loop, rolling windows, history, notifications, auto mode
  trends.rs       rolling series, linear fit, growth and CPU readings
  notify.rs       native notification delivery
  service.rs      launchd install/uninstall/restart/status
  app.rs          find the menu bar app (for `autotrim open`) and the CLI
                  binary (for the app's "Run in background")
  watch.rs        live terminal view and log printing
  paths.rs        data directory per platform
  report.rs       text rendering
  fmt.rs          bytes, durations, dates
  lib.rs          the Snapshot type and take_snapshot; everything above is a module
tray/
  src/main.rs     Tauri menu bar app: tray menu, window, commands
  ui/index.html   the window, plain HTML and JS, no bundler
  tauri.conf.json window and bundle settings
  icons/          app icon; icon-source.png is the 1024px original
scripts/
  install-app.sh  bundle the tray as autoTrim.app and put it in Applications
```

The root package is both the `autotrim` library and the `autotrim` binary;
`tray/` is a second package in the same workspace that depends on the
library. `cargo build --release` at the root builds the CLI; the tray is
`cargo build -p autotrim-tray --release`.

Everything upstream of `rules.rs` produces one `Snapshot` struct, serialized
to JSON. The daemon, the tray, the MCP server, and the `scan` command all
consume that struct. Platform-specific code stays behind `cfg` in `system.rs`,
`browser.rs`, `paths.rs`, `openfiles.rs`, `service.rs`, and `groups.rs`; the
path rules in `groups.rs` are plain functions tested on every host.

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

- `autotrim close <pid>` and `autotrim stop <pid>`: the reclaim verbs. `close`
  takes a fresh snapshot, refuses anything that is not a detected agent
  session, refuses the session running the command, refuses an active one
  without `--force`, prints the resume command, sends the polite signal,
  waits up to ten seconds, and only then kills. `stop` does the same for a
  listening process and refuses one that belongs to an app, a system
  process, or an agent session without `--force`. Both take `--dry-run`.
  Every action is appended to `actions.jsonl` in the data directory with
  what it was, what it held, and how to get it back; `autotrim actions`
  prints that log.
- **Auto mode**, off by default. Switch it on from the window (the Auto
  mode card on the Overview: close stale sessions, stop old servers, dry
  run), from the menu bar menu, with `autotrim config set
  auto_close_sessions=true`, or by editing `config.toml`; the daemon
  re-reads the file when it changes, so nothing needs a restart. When on,
  the daemon warns first ("closing N idle targets in 10 minutes", one
  notification), waits the grace period, and acts only on targets that
  are still idle then. Its bar is higher than the advice's: a session
  needs transcript evidence of idleness, a warm quiet window agreeing, a
  host on the `auto_hosts` allowlist, and it always spares the most
  recently active session in each project so you keep your place. Codex
  sessions are never targets: the app that owns them restarts them.
  Servers are only stopped when the owner is a known dev runtime (node,
  python, ruby, and friends); a VM manager or database left running is
  reported, never killed. `auto_dry_run = true` logs and notifies what it
  would have done, and does nothing, which is how to try it for a week;
  each target is reported once, not again every grace period, until it
  goes away or the dry run ends. The snapshot carries what auto mode is running with and every target it
  has warned about, so the window, `autotrim status` and the menu show
  "closing X in 7 m" rather than leaving the notification as the only
  trace. Switching auto mode off empties that list, so switching it on
  again starts every grace period afresh.
- **Trends.** The daemon keeps a rolling series per app and per session
  (two hours by default, warmed from the history files on restart, so a
  restart forgets nothing) and fits a line through each. Three rules read
  them. *Leak-like growth*: memory rising steadily, meaning a slope above
  200 MB/h, at least 150 MB gained, most samples rising, and a good linear
  fit, so a build or a page load does not trigger it. *Sustained CPU*: mean
  above 90% of a core for ten minutes with the minimum never far below,
  so bursts do not count. *Pressure rising*: swap climbing faster than a
  gigabyte an hour, with the fastest-growing apps named as the likely
  cause. Growth is advice, never an action; a leak and a legitimately busy
  program look the same from outside. `scan`, `status`, and the window
  show a Trends table once the daemon has ten minutes of history. This is
  the part macOS does not do at all: Activity Monitor shows an instant,
  never a direction, and never says what changed.
- **Browser tabs.** Every open tab in every running Chrome profile (and
  Chromium, Brave, Edge, Vivaldi): title, URL, site, profile, pinned, and
  how long since you last looked at it. This comes from the browser's own
  session files, the ones it would restore from after a crash, found by
  looking at which of them the browser process holds open, so nothing asks
  the browser anything and closed profiles do not count. Tabs roll up by
  site into a worst-offenders list, and a tab not looked at for a day
  (`tab_stale_after_hours`) is stale; the browser rule now also fires at
  fifteen stale tabs (`browser_stale_tabs`) and names the worst sites.
  `autotrim tabs` lists them longest-untouched first with their ids, and
  `autotrim close-tab <id>…` closes them. Per-tab memory is not something
  stable Chrome publishes; the ≈ figure everywhere is renderer memory
  divided by open tabs, and is labelled as an estimate.
- **Conversation pages, and pages that grow.** Tabs are classed by site:
  chatgpt.com, claude.ai, gemini.google.com and the other chat UIs are
  conversations, localhost and friends are local apps, the rest are pages.
  A conversation page keeps the whole exchange in the DOM and grows with
  it, and a background tab never gives that back, so the report, the
  window and `autotrim tabs` mark them, the browser view has a "Close N
  stale conversations" button, and a rule fires at two stale conversation
  tabs (`chat_stale_tabs`); the services keep the history, so closing
  loses nothing. The daemon also follows every tab renderer on its own,
  and a page that grows steadily gets its own advice card, with the
  long-lived pages that are open listed as the candidates. Chrome does not
  say which tab a process is, and autoTrim says so rather than guessing.
- **Session names.** A session is shown by the title you gave it, when the
  transcript records one, otherwise by the first thing you asked, shortened
  to a line, and only then by the agent's own derived label. Transcripts are
  append-only, so each one is read in full once and then only what was
  appended since.
- **Which agents.** Claude Code (the CLI, under the Claude app, VS Code, or a
  terminal), Codex (the CLI, and the app server the ChatGPT app and the VS
  Code extension run), Copilot CLI (from npm or Homebrew, and the copy VS
  Code bundles and runs as its engine), Cursor's CLI (`agent`, which is a
  launcher around its own bundled node), plus Gemini CLI, Aider, OpenCode,
  and OpenClaw by name. Codex, Copilot, and Cursor sessions are tied to
  their transcripts through the process's own open files: Codex rollouts,
  Copilot `session-state` event logs, Cursor chat stores. Codex threads are
  named from its thread index, Copilot sessions from `workspace.yaml`,
  Cursor chats from the chat store's own name, each falling back to the
  first prompt. An app's engine (Codex `app-server`, Copilot `--server`) is
  one session serving every thread the app shows; with nothing open it is
  part of the app, not a session, and auto mode never closes an engine
  because the app would restart it. Closing any of them logs the resume
  command: `claude --resume`, `codex resume`, `copilot --resume=`,
  `agent --resume`.
- `autotrim quit <app>` asks an application to quit the way ⌘Q would, so it
  can prompt to save or refuse. It never quits an app that hosts agent
  sessions without `--force`, never the app running the command, and never
  the Finder or the tray itself. Like the other verbs it is logged with the
  command to reopen the app.
- **The tray app** (`tray/`, a separate binary in the same workspace): a
  menu bar item showing free memory, with a menu that carries the summary
  line, the current advice, "Close N stale sessions", and "Open autoTrim…".
  The window is a sidebar of everything holding memory, largest first, with
  a bar for its share of RAM and a count of what it contains (sessions,
  tabs, processes). Click one for the detail: an agent's sessions by name
  with a Close on each and "Close N stale"; a browser's worst sites and
  every tab, longest untouched first, filterable, with Close per tab, per
  site, and for every stale tab at once; an app's memory, trend, hosted
  sessions and ports, with a Quit. Overview carries the advice (each card
  links to the view it is about), the largest holders, and trends. Above
  them sit two cards. *Auto mode* has the three switches and, once the
  daemon has picked them up, the targets it has warned about with the
  time left on each. *In the background* says whether the daemon runs as
  a login service and has the "Run in background" button that installs it
  (with the `autotrim` binary next to the app, inside its bundle, or on
  PATH), a "Hide window" button, and the choice of whether the window
  opens when the app starts (`open_window_at_launch`). Buttons
  are two-step: first click arms, second click acts, and the result with
  its resume command appears in a toast and in the Actions list. The tray
  reads the daemon's snapshot every five seconds and only scans on its own
  when no daemon is running. No Dock icon. Launching it again only brings
  the window forward. The window is created when you open it and destroyed
  when you close it, so an idle tray is only the menu item. Built with
  Tauri on the system web view: measured at about 60 MB resident idle on
  macOS, which is the runtime's price, against the daemon's 7 MB. A future
  pure-tray build without a web view could get that under 15 MB; the
  dashboard would then open in the browser instead.
- `autotrim config`: print the effective settings and where they came from.
  `autotrim config init` writes `config.toml` in the data directory with
  every setting, its default, and a comment. Flags override the file, the
  file overrides the defaults. Thresholds, intervals, notification cadence,
  and ignore lists for ports, apps, and projects all live there.
  `autotrim config set key=value …` changes settings in place and keeps
  the file's comments; the window's switches go through the same code.
  The daemon re-reads the file whenever it changes.

Every report also carries whole-machine CPU and load average, CPU per app
group and per session, and a table of listening TCP/UDP ports with the app
group or agent session that owns each one, how long it has been open, and
what it is. Labels come from three sources, cheapest first and none of them
a guess: a table of ports and owners people recognise (AirPlay, Handoff,
Lima, Spotify, Postgres, the VS Code extension host), the owner's own
command line and working directory ("Next.js dev server · ~/code/shop"),
and, for what is still vague, one short HTTP request to the port itself,
which fingerprints Vite, Next.js, Express, Flask, Django, uvicorn and
friends from their headers and markup. Probes go to loopback only, time out
in 300 ms, skip ports that speak something other than HTTP, and the daemon
remembers each answer so a port is asked once in its lifetime. A port that
none of this can name stays a question mark rather than a story.
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
- **No per-tab memory.** Chrome exposes renderer-to-tab mapping only
  inside itself (its task manager) or over the DevTools protocol, which
  needs a launch flag. Tabs are attributed by count and age, and memory per
  tab is an average. Discarding a tab (Memory Saver's trick) is likewise
  not reachable from outside; closing is. The daemon does follow each
  renderer's growth on its own, which shows a leaking page without naming
  it.
- **Tab closing is macOS only** for now: it is an Apple Event to the
  browser, the same as pressing ⌘W in that tab. Linux and Windows list tabs
  but cannot close them yet.
- **Codex sessions cannot be closed usefully.** The Codex process is a
  server owned by the ChatGPT app or VS Code, which restarts it. Auto mode
  never targets it; `close` will, with `--force`, and it will come back.
- **Auto mode never closes the only stale session in a project.** Sparing
  the most recently active session per project is what keeps your place,
  but a project with one forgotten session keeps it forever. A horizon
  (spare it only while it is less than a day idle, say) would fix that.
- **Auto mode's grace restarts on any one-tick wobble.** A target leaves
  the pending list the moment it fails a single check, and a session whose
  process tree idles at about 2% CPU (Claude Code with a few MCP servers
  under it) crosses the quiet threshold now and then. It should take a
  real burst, or a transcript update, to spare a target.
- **No quiet hours** for notifications yet; the config file is where they
  will go.
- **macOS only** for the service and the memory counters beyond swap.
  Linux and Windows build in CI and run `scan` and `daemon`: app grouping
  uses each platform's install layout, browsers are found by their native
  executable names, and Codex attribution needs the open-file table, which
  Windows does not expose yet. Neither has been used on a real desktop.

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

The menu bar app, as a real application with an icon:

```bash
./scripts/install-app.sh
```

That bundles the tray with Tauri and copies `autoTrim.app` into
Applications (yours, when the system folder is not writable). Open it from
there, keep it in the Dock, or from any terminal:

```bash
autotrim open
```

`open` launches the app or brings its window forward. The app refuses to
run twice, and so does the daemon: one per data directory, tracked in
`daemon.pid`, with a stale file from a crash ignored. The app runs until
you pick Quit from its menu. Its window can install the login service
too ("Run in background" on the Overview; `install-app.sh` puts a copy of
the command-line binary inside the bundle for that), and
`open_window_at_launch = false` makes the app start as a menu bar item
only. Starting the app itself at login and signing the bundle are still
to do. For a quick unbundled run while developing,
`cargo run -p autotrim-tray --release` still works.

## Decisions

Things that came up and where they landed.

- **Renderers are not tabs.** Chrome runs a process per tab, but also one
  per cross-site frame (site isolation), per prerendered page, plus a spare
  it keeps warm, so 52 renderer processes for 31 tabs is normal. The tab
  and window count comes from the browser's own session files, so the
  report says both numbers. Where those files cannot be read it splits
  renderers into tab-sized and small and the browser rule thresholds on
  the tab-sized count. Asking the browser over AppleScript was tried and
  dropped: it prompts for permission, and the session file already knows.
- **Which tab is which process is not guessable, so it is not guessed.**
  Every memory tool sees "Google Chrome Helper (Renderer)" fifty times and
  cannot say which is the abandoned chat and which is the page doing work.
  Chrome only maps renderers to tabs inside itself or over a debugging
  port it does not open by default, and site isolation means the count of
  renderers is not the count of tabs either. autoTrim reports three things
  it can know: what each tab is and when it was last looked at (the session
  file), which pages are the kind that grow with use (conversation UIs,
  local apps), and which renderer processes are growing (the daemon's
  series). Together they point at the page without naming it, and the
  stale conversations are safe to close either way.
- **Port labels are deterministic, not model-guessed.** The idea of asking
  a local model what a port is came up. The command line, the working
  directory, and a one-request HTTP fingerprint identify nearly every dev
  port precisely for free, and a model would be guessing from the same
  evidence with less rigour. The leftovers are exactly what the planned
  MCP interface is for: your own agent, with the snapshot in hand, can go
  and look.

- **Closing a Claude Code session is clean.** Tested on a VS Code-hosted
  session that had been idle for 54 days: the polite signal was enough, the
  process exited within a second, Claude Code removed its own session file,
  the transcript stayed on disk, and VS Code did not respawn it. The resume
  command in the action log brings it back.

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
- **The UI is a Tauri tray app** that reads the daemon's files rather than
  talking to it over a socket. The daemon writes `latest.json` every tick
  and the action log is append-only, so a file is the simplest possible
  interface and the tray never needs the daemon to answer. The terminal
  `watch` view stays for people who live in a terminal.
- **Tabs come from Chrome's session files, not from Chrome.** The
  alternatives were AppleScript (a subprocess and an Automation prompt on
  every tick, for the daemon of all things) and the DevTools protocol (a
  launch flag nobody has set). The session file under each profile's
  `Sessions/` directory is an append-only log of everything session restore
  needs, written a couple of seconds after any change, and it carries the
  one thing neither alternative does: when each tab was last the active
  one. `snss.rs` reads it with no dependencies and skips commands it does
  not know, which is how it survives new Chrome versions. Which profiles
  are live is answered by which session files the browser process holds
  open. The tab id in that file is the same number the browser's
  AppleScript dictionary reports, verified against a running Chrome, which
  is what lets a close target one exact tab; the browser re-checks the URL
  before closing, so a tab that moved on since the snapshot is left alone.
  One trap: Chrome hands that id over as text, and AppleScript's integer
  stops at 2^29, so an id near two billion coerced to integer silently
  becomes a real and matches nothing. The script compares ids as text.

