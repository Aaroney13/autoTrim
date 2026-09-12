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
- One `Snapshot` struct (src/main.rs) feeds every consumer. Add fields there
  and keep them serde round-trippable; `status` deserializes `latest.json`.
- Platform-specific code stays behind `cfg` in system.rs, browser.rs, paths.rs.
- The daemon's footprint is a public promise (under 20 MB resident). Check it
  when adding dependencies.
