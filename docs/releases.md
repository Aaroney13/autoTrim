# App release workflow

For standalone CLI archives and installation without Rust, see
[CLI releases](cli-releases.md). They use `cli-vVERSION` tags independently of
the app's `vVERSION` tags.

The macOS app workflow is `.github/workflows/release.yml`. Every push to `main`
(or a manual workflow run on `main`) builds and publishes an update automatically.
No version-bump commit, manual tag, or draft-publishing step is needed.

`scripts/prepare-release.mjs` chooses the next patch version above both the
source version and existing stable app tags/releases, including failed drafts.
It updates both Cargo packages, the lockfile, and Tauri configuration in the CI
checkout only. The generated `vVERSION` tag points to the exact source commit;
the version edits are build inputs, not additional commits on `main`. For example,
source version 0.2.0 produces 0.2.1, then 0.2.2 on the next main build.

The workflow checks formatting, Clippy, Rust tests, UI/release tests, and version
consistency, then builds a universal app containing the daemon. It uploads to a
draft first, downloads the uploaded updater feed/archive, and verifies that both
Mac architectures have a matching version, uploaded archive, and valid signature
against the app's existing public key. It also checks both binaries are universal
and smoke-tests the bundled CLI version. Only then does it publish and mark the
release latest, and check that the public update feed matches the verified one.

Release runs are serialized. If a newer commit arrives on `main` during a build,
the old build stays a draft and the queued newest build supplies the update.
GitHub may coalesce intermediate pending pushes. Failed runs leave the previous
published update available; fix the failure and push again or rerun on `main`.
Do not manually publish incomplete drafts or mark CLI releases as latest: the
installed app reads `/releases/latest/download/latest.json`.

In an installed app at version 0.2.0 or newer, open **Settings → App updates →
Check for updates**, then **Install and restart**. Settings and history stay in
place; the app, bundled CLI, and enabled background service update together.
The first automatic release works with existing 0.2.0 installations because
the update endpoint and public verification key are unchanged. Older apps need
one manual upgrade with `git pull && ./scripts/install.sh`, or the latest DMG.

App updater signatures require repository secrets `TAURI_SIGNING_PRIVATE_KEY`
and, if the key is encrypted, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Keep the
private key out of source control; the public verification key is configured in
`tray/tauri.conf.json`. Losing or changing the signing key affects existing
clients' ability to verify updates.

Optional Apple signing/notarization is configured with all of
`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
`APPLE_ID`, `APPLE_PASSWORD`, and `APPLE_TEAM_ID`. The workflow refuses partial
Apple configuration. With none set it uses ad-hoc signing for test builds;
Gatekeeper restrictions still apply and these builds are not notarized.

The Tauri updater verifies its artifact signature and the app performs update
checks at launch and periodically. Installation remains a user action in
Settings. A locally passing build does not verify GitHub secrets, notarization,
or an end-to-end installed-app update; the workflow verifies the uploaded
signatures before publication, and installation should also be tried on a Mac.
