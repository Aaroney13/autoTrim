# autoTrim

Rust CLI/daemon that monitors memory holders (apps, browsers, AI agent
sessions) and advises or reclaims. Scope and principles are in README.md;
read the "Out of scope" list before adding features. It is not an agent
orchestrator and the daemon never calls a model.

## Working here

- Toolchain was installed with rustup `--no-modify-path`: use
  `export PATH="$HOME/.cargo/bin:$PATH"` in shells that lack it.
- Before committing: `cargo fmt && cargo clippy` with zero warnings, then
  `cargo build --release` and run `./target/release/autotrim scan` once.
- Test the daemon against a scratch directory, never the real data dir:
  `AUTOTRIM_DATA_DIR=/some/scratch ./target/debug/autotrim daemon --once`.
- One `Snapshot` struct (src/lib.rs) feeds every consumer. Add fields there
  and keep them serde round-trippable; `status` deserializes `latest.json`.
- Platform-specific code stays behind `cfg` in system.rs, browser.rs, paths.rs,
  openfiles.rs, service.rs, groups.rs, automation.rs. Pure path rules
  (groups.rs) are compiled and unit-tested on every host; only the
  dispatcher is `cfg`-selected.
- Cross-check before pushing: `cargo clippy -p autotrim --target
  x86_64-unknown-linux-gnu` and `--target x86_64-pc-windows-msvc` (targets
  installed via rustup). CI builds all three platforms.
- Browser tabs are read from Chromium's session files (snss.rs), never by
  asking the browser. Only actions talk to other apps (automation.rs), and
  only when a person asked.
- The workspace has two packages: the root (`autotrim`, library + CLI) and
  `tray/` (`autotrim-tray`, Tauri). Lint with `cargo clippy --workspace`.
  The tray never depends on sysinfo directly; it goes through the library.
- Any interface that acts goes through `actions::close_by_pid`,
  `actions::stop_by_pid`, `actions::close_tabs_by_id`, or
  `actions::quit_app_by_name` so the safety checks and the action log are
  shared.
- The tray window (tray/ui/index.html) is embedded at compile time; rebuild
  the tray after editing it.
- The daemon's footprint is a public promise (under 20 MB resident). Check it
  when adding dependencies.
