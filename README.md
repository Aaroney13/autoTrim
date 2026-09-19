# autoTrim

A small always-on monitor for developer machines. It notices when memory is
being wasted, tells you what is holding it, and can safely reclaim it.

Dev machines fill up with things that hold memory while doing nothing: idle
AI agent sessions (Claude Code, Codex, Cursor, Copilot), a browser with
ninety tabs across five profiles, Electron apps left open for days, a machine
not restarted in two months. Activity Monitor shows a flat list of helper
processes and forgets what happened an hour ago. autoTrim samples
continuously, groups memory by the thing you would actually close, applies a
small set of rules you can read, and notifies you with a number and an
action attached.

## Install

For the CLI without Rust, use the published `cli-vVERSION` archives on
[GitHub Releases](https://github.com/Aaroney13/autoTrim/releases).
[Download, verify, install, upgrade, and remove](docs/cli-releases.md) documents
the four supported targets and unsigned-binary restrictions. If no CLI release
has been published yet, build from source below.

macOS, with [Rust](https://rustup.rs) installed (and Node, for the menu bar app):

```bash
git clone https://github.com/Aaroney13/autoTrim.git && cd autoTrim && ./scripts/install.sh
```

That builds the `autotrim` command and puts it on your PATH, starts the
daemon now and at every login, and puts autoTrim.app in Applications.
Starting with 0.2.0, the macOS app checks GitHub Releases at launch and
every six hours. Use **Settings → App updates → Install and restart** to
install a signed update, including its background daemon. The installed
command follows the app via a symlink, so it updates too.

Older installations need one manual upgrade: `git pull && ./scripts/install.sh`.
CLI-only installations still upgrade by running the installer after pulling.
One command takes it all off again, and your data stays:

```bash
./scripts/install.sh --uninstall
```

`--no-app` skips the menu bar app (then Node is not needed), `--no-service`
skips the login service, and `AUTOTRIM_BIN_DIR` picks where the binary goes
(`/usr/local/bin` when writable, else `~/.local/bin`). For just the command,
with cargo:

```bash
cargo install --path . && autotrim service install
```

Linux and Windows build and run `scan` and `daemon`. The login service, the
app, and the tab and quit verbs are macOS only for now.

Maintainers: [publishing updates](docs/releases.md) describes the signing
setup, version tags, and draft-release publishing step.

## Use

```
autotrim scan          one-shot report: totals, holders, sessions, tabs, ports, advice (--json)
autotrim watch         live terminal view of the daemon's snapshot and log
autotrim status        the daemon's latest snapshot, without sampling
autotrim open          the menu bar app's window
autotrim tabs          every open browser tab, longest untouched first
autotrim actions       what was closed, when, and how to get it back
autotrim config        the effective settings; `init` writes config.toml, `set k=v` edits it
autotrim service       install | uninstall | restart | status
autotrim log -f        follow the daemon log
```

The reclaim verbs, each with `--dry-run` and available recovery instructions:

```
autotrim close <pid>        close an idle agent session; logs the resume command
autotrim stop <pid>         stop an old local dev server
autotrim close-tab <id>…    close browser tabs by id, as `tabs` lists them
autotrim quit <app>         ask an app to quit, as ⌘Q would
autotrim restart <app>      quit, wait, reopen; browser session restoration is best effort
```

`close` refuses anything that is not a detected agent session, the session
running the command, and an active session without `--force`; it sends the
polite signal, waits up to ten seconds, escalates surviving targets, and verifies
the result. Partial failures remain visible in Actions. `stop` refuses a process
that belongs to an app, the system, or an agent session without `--force`.
`quit` and `restart` never touch an app hosting agent sessions without
`--force`, the app running the command, the Finder, or the tray.

**The menu bar app** shows free memory, and its menu carries the summary,
the current advice, "Close N stale sessions", and "Open autoTrim…". The
window lists everything holding memory, largest first: an agent's sessions
with a Close on each, plus its own app's ports, Quit, and Restart when that
app is running; a browser's worst sites and every tab with Close per tab,
per site, or for every stale tab at once; an app's trend and ports with Quit
and Restart. Session and tab lists support search and filters; batch
actions apply to the displayed results. Closing opens a target review,
with recovery instructions saved in Actions. Server stops also use a target
review; Quit and Restart arm on the first click and act on the second. Settings offers
Off, Preview only, and On for auto mode, plus "Run in background", which
installs the login service. The app reads the daemon's files and scans
when no daemon is running, on Refresh, and after actions. Commands use the
shared library directly. It is Tauri on the system web view,
about 60 MB idle in earlier measurements. The daemon targets under 20 MB;
see the measured limits in [design notes](docs/design.md#transcript-activity-cache).

**Auto mode** is off by default. Turn it on in the window, from the menu,
or with `autotrim config set auto_close_sessions=true`; the daemon re-reads
`config.toml` when it changes. When on, the daemon warns first ("closing N
idle targets in 10 minutes"), waits the grace period, and acts only on
targets still idle then. Its bar is higher than the advice's: transcript
evidence of idleness, a quiet CPU window agreeing, a host on the
`auto_hosts` allowlist, and the most recently active session in each
project is always spared. Servers are only stopped when the owner is a known
dev runtime. Whenever auto mode is on, empty Chrome New Tab pages are also
closed after the warning period. Selected and pinned tabs are spared;
visiting a tab or navigating away from New Tab cancels its pending close.
`auto_dry_run = true` logs what it would have done and does
nothing, which is how to try it for a week. Every target is checked again
against fresh activity, process identities, exclusions, and current settings
immediately before execution.

Data lives in `~/Library/Application Support/autotrim` on macOS,
`$XDG_DATA_HOME/autotrim` on Linux, and `%LOCALAPPDATA%\autotrim` on Windows
(`AUTOTRIM_DATA_DIR` overrides it): `latest.json`, daily history, the action
log, `config.toml`, and `daemon.log`.

**Privacy:** snapshots contain prompt excerpts, full tab URLs, and browser
profile details. Data stays local; Unix files are private to your user. Logs
rotate at 5 MiB with three backups. The [privacy statement](PRIVACY.md) lists
what is read and saved, network requests, retention, permissions, and deletion.
Storage failures retry on later ticks and notify you; fatal errors and Rust
panics produce a local `crash.json` when storage is available.

Before any real cleanup, the shared journal durably saves an action ID, target
identity, and available recovery details. A failed intent write prevents execution.
Completion distinguishes success, failure, and partial results; an interrupted
intent stays marked incomplete. Reopening a URL or resuming a transcript cannot
guarantee restoration of unsaved drafts, temporary chats, or in-memory work.

## What it sees

- **Memory currently held.** macOS figures use `phys_footprint`, the counter
  behind Activity Monitor's Memory column. Helpers roll up under their app.
  These are pre-action footprints, not measured savings; Linux/Windows RSS
  can count shared pages more than once.
- **Agent sessions.** Claude Code (the CLI, the Claude app, VS Code, a
  terminal), Codex (the CLI and the app server behind the ChatGPT app and VS
  Code), Copilot CLI, Cursor's CLI, plus Gemini CLI, Aider, OpenCode, and
  OpenClaw by name. Each with its host, project, name (your title, else your
  first prompt), age, idle time, and memory. The agent's own client (the
  Claude app, ChatGPT) is folded into the group, so it is not listed beside
  its sessions as a second holder. Codex backends expand into the tasks whose
  transcripts they hold open, with titles, projects, last activity, and labeled
  helpers. Memory and CPU remain shared at the backend; saved history is not
  counted as loaded tasks. Closing a session logs the resume command.
- **Browser tabs** in every running Chrome, Chromium, Brave, Edge, and
  Vivaldi profile: title, site, pinned, and how long since you last looked,
  read from the browser's own session files rather than by asking it.
  Conversation pages (chatgpt.com, claude.ai, and friends) are flagged,
  because they can grow with use. Inactive tabs may already be deactivated.
  Per-tab estimates divide renderer memory by all tabs; site estimates
  multiply that average by the selected count. Age does not prove retained memory.
- **Trends.** A rolling series per app and per session, warmed from history
  on restart, with rules for leak-like growth, sustained CPU, and swap
  rising, naming the fastest-growing apps. Growth is advice, never an action.
- **Ports.** Every listener with its owner, its age, and what it is, from a
  known table, the command line, or one 300 ms HTTP request to loopback.
  Old unmanaged dev servers get their own advice.

How "idle" is decided, strongest evidence first:

1. Busy window-mean CPU means active at the configured threshold (default
   2%). With transcript evidence, a one-shot sample must reach the larger
   of that threshold and 10% to override it.
2. Claude Code's transcript tail gives the last real exchange, so a session
   idle past the stale threshold (6 h by default) is stale, even on a
   one-shot scan.
3. Codex's rollout files, found through the server's own open files, do the
   same for Codex.
4. Anything else must be observed quiet by the daemon for 15 minutes.
5. A one-shot scan of an agent with no transcript falls back to age plus a
   quiet sample, and the JSON labels it as the weak signal it is.

## Scope

In scope: observe cheaply and continuously; attribute memory to agent
sessions and browsers as first-class groups; advise with deterministic
rules; notify with rate limits; reclaim with a few narrow, logged,
cleanup verbs. The CLI supports one-shot and continuous operation; the tray
is a separate binary. An MCP/localhost interface is future work.

Out of scope, deliberately:

- **Orchestrating agents.** No starting sessions, no sending prompts, no
  plans, no queues. autoTrim only ever stops things or suggests stopping them.
- **Being an agent.** No model calls inside the daemon. Every decision is a
  rule you can read in the source.
- **Memory "cleaning."** No purge tricks, no cache clearing, no claims that
  freeing inactive memory speeds anything up.
- **General system monitoring.** No CPU graphs, fans, network, or disk
  dashboards beyond what the memory rules need.
- **Arbitrary process killing.** Actions are named verbs on recognized
  targets, never a generic kill.

Principles: a daemon footprint under 20 MB as Activity Monitor counts it,
and it logs its own number; the same snapshot always gives the same advice,
with the thresholds in one config file; every action is logged with available
recovery instructions; never surprise, so auto mode is off by default, warns first, and
spares the session you are sitting in; the daemon and the agent session it
runs under are excluded from its own actions; open source, normal user
permissions, one command to uninstall.

## Limits

- Per-tab memory is not something Chrome exposes; tabs get a count, an age,
  and an average.
- Closing tabs, quitting, and restarting apps are macOS only (Apple Events).
- Host-managed Codex/Copilot engines are excluded from auto mode and need
  `close --force`. Standalone Codex CLI sessions follow the same activity
  and host restrictions as other standalone sessions.
- Notifications have no buttons and no quiet hours yet.
- The localhost interface for your own agent is planned, not built.
- Linux and Windows build in CI and have not been used on a real desktop.

## Development

```bash
cargo build --release && ./target/release/autotrim scan
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
node --test tests/tray-ui.test.mjs                                     # UI action/filter checks
AUTOTRIM_DATA_DIR=/tmp/scratch ./target/debug/autotrim daemon --once   # never the real data dir
cargo run -p autotrim-tray --release                                   # the app, unbundled
```

For browser interaction checks with synthetic data, install Playwright with
`npm install --no-save --package-lock=false playwright`, run
`npx playwright install chromium`, then `node tests/tray-interactions.browser.cjs`.
The checks cover full-row clicks and highlighting, keyboard and checkbox
selection, and clicks during background refresh in light and dark themes.
`AUTOTRIM_TEST_CHROMIUM` can point to an existing Chromium executable.

Everything upstream of `rules.rs` produces one `Snapshot` (`src/lib.rs`),
serialized to JSON, and the daemon, the tray, `scan`, and `status` all
consume it. Roughly: `system.rs`, `procs.rs`, `footprint.rs`, and `groups.rs`
measure and group; `agents.rs`, `transcripts.rs`, and `openfiles.rs` find
sessions; `browser.rs` and `snss.rs` read tabs; `ports.rs` and `portlabel.rs`
name listeners; `trends.rs` fits the series; `rules.rs` turns all of it into
advice; `daemon.rs` runs the loop; `actions.rs` and `automation.rs` are the
verbs; `service.rs`, `notify.rs`, and `paths.rs` are the platform edges.
The app uses compact lists with a details pane for sessions, tabs, sites, ports,
and memory trends. Stale rows show a small idle duration beside their status dot.
Selections follow the visible filters, and protected items cannot be selected.
The details pane stacks below the list in narrow windows.

`tray/` is the Tauri app. Its HTML, stylesheet, and ES modules in `tray/ui/`
are embedded at compile time; no frontend server or network is required. Platform-specific code stays behind `cfg`, and CI builds
macOS, Linux, and Windows.

The reasoning behind the design, the fine print on each part, and the full
list of known gaps are in [docs/design.md](docs/design.md).
