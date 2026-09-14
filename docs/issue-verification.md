# Open issue fixes: local verification

Implementation work for the ten open issues reviewed on 2026-09-14 lives on
`codex/fix-open-issues`. This checkout also contains concurrent app-updater,
privacy, diagnostics, and storage changes; they were preserved and the combined
workspace was tested. This work does not publish a release or close GitHub issues.

| Issue | Implementation and evidence |
| --- | --- |
| [#1](https://github.com/Aaroney13/autoTrim/issues/1) | Table-driven CPU, transcript, age, warm-up, exact-threshold, and contradictory-evidence tests in `rules.rs`. Documentation now distinguishes immediate transcript classification from stricter auto eligibility. |
| [#2](https://github.com/Aaroney13/autoTrim/issues/2) | Pure `policy.rs` decisions with explicit time, persisted pending/preview state, candidate exclusions, newest ties, server deduplication, cancellation, restart and preview/live lifecycle tests. |
| [#3](https://github.com/Aaroney13/autoTrim/issues/3) | Shared durable intent/completion journal for all action types, stable IDs, failed-write suppression, incomplete interrupted actions, legacy/rotated-log merging, dry-run and fault-injection tests. |
| [#4](https://github.com/Aaroney13/autoTrim/issues/4) | Structured signal delivery and exit results for each identity; bounded graceful/hard-kill phases; survivors and changed identities remain visible. Fake process adapter tests; no real sessions killed. |
| [#5](https://github.com/Aaroney13/autoTrim/issues/5) | Shared process execution boundary, reviewed root/start time, descendant identity checks, fresh per-target automatic sampling/configuration, preserved daemon quiet evidence, explicit skips. Tests cover disappearance, renewed activity, reuse and mode changes. |
| [#6](https://github.com/Aaroney13/autoTrim/issues/6) | Notifications and UI distinguish previous footprint from measured savings, disclose tab/site averaging, acknowledge deactivated tabs and unsaved state, and retain failed outcomes. Formatting/estimate tests included. |
| [#7](https://github.com/Aaroney13/autoTrim/issues/7) | Bounded LRU activity cache, parser/file identities, append handling and backward history scanning; synthetic replacement/truncation/deletion/partial-line tests and a repeatable benchmark. Limits are documented below and in design notes. |
| [#8](https://github.com/Aaroney13/autoTrim/issues/8) | HTML/CSS/ES-module split with explicit dependencies and injected renderer callbacks. Eighteen UI tests plus a browser fixture verify loading, views, filtering, confirmation, failures, polling and same-second snapshot ordering. |
| [#9](https://github.com/Aaroney13/autoTrim/issues/9) | README/design consistency pass, implemented stop, managed engines vs standalone CLIs, journal lifecycle, tray file/library access, future MCP/localhost scope, and honest memory/recovery wording. Local Markdown file links checked. |
| [#10](https://github.com/Aaroney13/autoTrim/issues/10) | Four-target version-tagged CLI draft workflow, locked builds, tag validation, notices/checksum packaging, extracted native smoke checks, rerun behavior and installation docs. Apple-silicon archive verified locally; all-runner CI/draft creation still needs a pushed `cli-vVERSION` tag. |

## Checks run

- `cargo fmt --all --check` and `git diff --check`.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`.
- `cargo test --workspace --locked`: 81 library tests and one tray test passed;
  the large transcript benchmark is separately ignored in the normal run.
- `node --test tests/tray-ui.test.mjs`: 18 passed.
- CLI clippy checks for `x86_64-unknown-linux-gnu` and
  `x86_64-pc-windows-msvc` passed. These are cross-target compile/lint checks,
  not native execution on those systems.
- Release CLI build, read-only `scan`, and `daemon --once --no-notify` against
  a dedicated scratch data directory. The daemon reported 7 MB current and
  peak footprint on one tick with six observed sessions; auto mode was off.
- Native `aarch64-apple-darwin` packaging verified extracted file hashes,
  `--version`, and `--help`. Workflow YAML parsed successfully; invalid
  version tags are rejected before building.
- In-app browser loaded the actual ES modules with the fixture IPC bridge;
  overview, holder details, tabs/sites, ports, settings, empty filters,
  two-step quit, reviewed close, partial failure, Actions, polling, and a
  fresh page after close/reopen were exercised. No console errors remained.

## Limits still requiring external validation

GitHub runners must execute the four-target release matrix before the release
issue can be considered verified end to end. Draft publication remains a
maintainer decision. Native Tauri window destruction/recreation and installed-app
updates were not exercised; browser close/reopen verifies module initialization,
not every native window lifecycle behavior. Signing/notarization are separate
from CLI packaging.

The transcript benchmark read a 136,192,011-byte file once across 100 checks;
unchanged checks read no content, a new activity append read 11 bytes, and the
tracked peak buffer was 65,600 bytes. The isolated benchmark process reported
2,245,064 bytes of peak `phys_footprint` (about 2.1 MiB). The whole-daemon 20 MB target is not a
worst-case guarantee. Oversized transcript lines return unknown activity; see
[design notes](design.md#transcript-activity-cache) for that limit and the
append/replace writer assumptions. PID start-time checks cannot atomically remove
the remaining OS race between inspection and signal delivery.
