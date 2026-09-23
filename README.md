# autoTrim

A local memory monitor for developer machines. autoTrim groups memory by app,
AI agent session, browser, and dev server, spots idle targets and growing memory
use, and helps you close them from a macOS menu bar app or the CLI.

- See agent sessions, browser tabs, listening ports, and memory trends.
- Close idle sessions and tabs, stop dev servers, or quit and restart apps.
- Review every cleanup in the action log, with available recovery instructions.
- Enable optional automatic cleanup with idle checks and a warning period.
  Auto mode is off by default.

## Install

On macOS, install [Rust](https://rustup.rs) and Node, then run:

```bash
git clone https://github.com/Aaroney13/autoTrim.git
cd autoTrim
./scripts/install.sh
```

This installs the `autotrim` command and autoTrim.app in Applications. Open the
app to choose what to monitor, idle thresholds, notifications, and whether to
run at login. The wizard explains empty-new-tab cleanup, offers optional Google
and social-site whitelist suggestions, and can review your idle Chrome tabs.
Revisit these choices in **Settings → Review setup**.

autoTrim appears in the Dock while running. Click its Dock icon to open the
dashboard again after closing it. To keep the icon after quitting, choose
**Options → Keep in Dock** from the icon's shortcut menu.

The app checks for updates automatically. To update manually, use
**Settings → App updates → Check for updates → Install and restart**.
Older installations and CLI-only installs can update with
`git pull && ./scripts/install.sh`.

Use `./scripts/install.sh --no-app` for a CLI-only install (no Node needed), or
`--no-service` to skip the login service. CLI-only installs start monitoring
immediately and at login by default. Uninstall with
`./scripts/install.sh --uninstall`; your data stays.

For prebuilt CLI archives, supported targets, and verification instructions,
see the [CLI installation guide](docs/cli-releases.md).
Linux and Windows support `scan` and `daemon`; the app, login service, and
browser-tab and app cleanup commands require macOS.

## Use

```bash
autotrim scan                 # One-shot memory report (--json supported)
autotrim watch                # Live view of the daemon's snapshot and log
autotrim open                 # Open the menu bar app
autotrim tabs                 # Browser tabs, longest untouched first
autotrim actions              # Cleanup history and recovery instructions
autotrim config               # View settings; use `set k=v` to edit
autotrim service status       # Also: install, uninstall, restart
```

Cleanup commands support `--dry-run`:

```bash
autotrim close <pid>          # Close an idle agent session
autotrim stop <pid>           # Stop a local dev server
autotrim close-tab <id>       # Close a tab listed by `autotrim tabs`
autotrim quit <app>           # Ask an app to quit
autotrim restart <app>        # Quit and reopen an app
```

With the optional [Codex desktop bridge](experiments/codex-bridge/README.md),
open an eligible task’s details and choose **Archive task**. Restore it from
Actions. Enable **Archive idle Codex tasks** in Settings for timed archiving
after the configured session inactivity threshold and warning period. The newest
task in each project stays open, and the shared backend keeps running.

The app provides search and filters. Tabs close directly from the selection or
details; sessions and servers have a target review before closing. Settings offers **Off**, **Preview only**, and **On** for
automatic cleanup. Auto mode warns first and rechecks activity before acting.

**Chrome tab cleanup** in Settings adds an optional domain allowlist, with exact
or subdomain matching and a read-only preview. Listed tabs wait 24 hours since
their last selection by default; presets range from 10 minutes to 1 week, followed
by the shared warning period. The list starts empty and adding a domain does not
enable closing. Selected, pinned, and unknown-activity tabs stay open. Background
page work and unsaved drafts cannot be detected.

Actions save recovery details before execution, but cannot guarantee restoration
of unsaved drafts or in-memory work. Memory figures describe current usage,
not guaranteed savings; per-tab memory is an estimate.

Data stays local. Snapshots include prompt excerpts, full tab URLs, and browser
profile details. On macOS, data lives in
`~/Library/Application Support/autotrim`; `AUTOTRIM_DATA_DIR` overrides the
location. See the [privacy statement](PRIVACY.md) for storage, permissions,
and deletion details.

## Development

```bash
cargo build --release
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
node --test tests/tray-ui.test.mjs
cargo run -p autotrim-tray --release
```

For browser checks, install Playwright with
`npm install --no-save --package-lock=false playwright` and
`npx playwright install chromium`, then run
`node tests/tray-interactions.browser.cjs` and `node tests/onboarding.browser.cjs`.
Use a scratch data directory when testing the daemon:
`AUTOTRIM_DATA_DIR=/tmp/scratch ./target/debug/autotrim daemon --once`.

See [design notes](docs/design.md) for architecture, cleanup safeguards, and
known limits, and [publishing updates](docs/releases.md) for signing and releases.
