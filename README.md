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
run at login. Revisit these choices in **Settings → Review setup**.

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

The app provides search, filters, and target review before closing sessions,
tabs, or servers. Settings offers **Off**, **Preview only**, and **On** for
automatic cleanup. Auto mode warns first and rechecks activity before acting.

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
