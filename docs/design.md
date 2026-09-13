# autoTrim design notes

The long version of the README: the plan, how each part works in detail,
how "idle" is decided, what is measured, the known gaps, and the decisions
that came up along the way. The README has the short version, the scope,
and the principles.

## The plan

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

## How each part works

- **Auto mode**, off by default. Switch it on from the window (the Auto
  mode card in Settings: Off, Preview only, or On, with target switches
  for stale sessions and old servers), from the menu bar menu, with `autotrim config set
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
- `autotrim restart <app>` quits the same way, waits until every process of
  the app is gone, then opens the bundle again, so a browser or an Electron
  app that has been up for days comes back holding a fraction of what it
  did. Chrome, Chromium, Brave, Edge and Vivaldi are opened with
  `--restore-last-session`, so the tabs come back as you click them rather
  than all at once. An app still running after thirty seconds (a save
  dialog, a refusal) is not relaunched, and the log says so; an executable
  that does not live in an application bundle is refused, since there is
  nothing to open again. macOS only, like the other Apple Event verbs. The
  same checks as `quit`, and the same log entry, with the `open` command.
- **The tray app** (`tray/`, a separate binary in the same workspace): a
  menu bar item showing free memory, with a menu that carries the summary
  line, the current advice, "Close N stale sessions", and "Open autoTrim…".
  The header is one physical-memory bar (used, with compressed as a second
  shade), a separate swap-on-disk figure, the CPU figure, and the age of the
  numbers with a ring that fills toward the daemon's next snapshot. The
  sidebar lists everything holding memory, largest first, with a count of
  what each contains (sessions, tabs, processes). Click one for the
  detail: an agent's searchable sessions with state filters and expandable
  CPU, age, host, PID, and ports; a browser's searchable tabs and site
  summaries with profile and state filters. Batch reviews apply to the
  displayed results; pinned and active tabs remain protected. Stale-tab
  filters use the configured threshold. The existing app trends, hosted
  sessions, ports, Quit, and Restart remain available. Overview puts advice
  first, separates lower-severity growth/CPU observations, and shows memory
  by kind and trends. The sidebar carries background and auto-mode status;
  pending auto closes also appear on Overview. Settings holds two cards.
  *Auto mode* offers Off, Preview only, and On, with session/server target
  switches and, once the daemon has
  picked them up, the targets it has warned about with the time left on
  each. Turning it off cancels pending closes when the daemon next reads
  the settings; the UI shows that applying state. *Run in background* says whether the daemon runs as a login
  service and has the "Run in background" button that installs it (with
  the `autotrim` binary next to the app, inside its bundle, or on PATH), a
  "Hide window" button, and the choice of whether the window opens when
  the app starts (`open_window_at_launch`). Session and tab closing opens
  a persistent review with exact targets, deselection, memory held, and
  recovery guidance. Polling does not replace that selection. The backend
  re-checks session start times and tab URLs/profiles against the reviewed
  identities, plus active/pinned status, before closing. Results and partial
  failures stay in the dialog; Actions has readable history and copyable
  recovery commands. Click a command or use Enter or Space on its Copy control
  to copy it. Successful copies show a green highlight and “✓ Copied!” for
  2.5 seconds; background refresh preserves that feedback, and reduced-motion
  settings disable the pulse. The browser’s alternative ⌘⇧T shortcut appears
  as helper text and is excluded from copying, including for existing logs.
  Other recovery hints stay intact. Tab memory is always labeled as estimated; memory
  held before an action is never described as measured savings. Other
  app and service actions retain their two-click confirmation. Server stopping
  uses the same target review dialog, with one entry per process. Sessions,
  tabs, sites, ports, and trends use compact lists with a details pane; stale
  rows show a small idle duration next to the status dot. Selection follows
  the visible filters and excludes protected items. The pane stacks below
  the list in narrow windows. The tray
  reads the daemon's snapshot every five seconds and only scans on its own
  when no daemon is running. No Dock icon. Launching it again only brings
  the window forward. The window is created when you open it and destroyed
  when you close it, so an idle tray is only the menu item. Built with
  Tauri on the system web view: measured at about 60 MB resident idle on
  macOS, which is the runtime's price, against the daemon's 13 MB footprint
  after eight hours with notifications on. A future
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

## How "idle" is decided

Strongest evidence first:

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

## What is measured

Memory counters on macOS come from `host_statistics64` and the
`kern.memorystatus_level` sysctl, the same sources `vm_stat` and
`memory_pressure` print, with no subprocesses. Per-process memory on macOS
is `phys_footprint` from `proc_pid_rusage`, the number Activity Monitor's
Memory column and `footprint(1)` show: private, compressed and IOKit
memory, not the pages shared with every other process. That is what a
process gives back when it exits, so a session's figure, and the recovery
on an advice card, is what closing it returns. Linux and Windows use
resident size from sysinfo, which charges shared pages to every process
and over-counts a tree.

## Known gaps

In the order they should be fixed:

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
- **Tab closing, quitting and restarting are macOS only** for now: Apple
  Events to the app, the same as pressing ⌘W or ⌘Q, and `open` to bring it
  back. Linux and Windows list tabs but cannot close them yet.
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

- **Footprint, not resident size.** Resident size charges the shared
  cache and every framework's text to each process that maps them, and
  never counts the pages the compressor is holding. So a tiny helper looked
  like 30 MB and a stale session on a swapping machine looked small, and
  summing a process tree double-counted the shared part. `phys_footprint`
  is what the kernel actually frees when a process exits, it is the column
  Activity Monitor shows, and the libc crate already bound the call. The
  field is still called `rss` in the JSON, because four on-disk formats and
  the tray read it by that name.
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
- The tray carries the approved preview’s navigation and search icons, section
  spacing, compact growth notices, and recovery cues. Primary navigation and
  background status stay visible while the holder list scrolls independently.

- Tables use alternating row shades in both themes, a distinct header, and
  row hover/focus highlighting. Session detail rows keep their parent’s shade;
  secondary labels stay at a readable size and contrast.

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
