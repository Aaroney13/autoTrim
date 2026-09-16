# App release workflow

For standalone CLI archives and installation without Rust, see
[CLI releases](cli-releases.md). They use `cli-vVERSION` tags independently of
the app's `vVERSION` tags.

The macOS app workflow is `.github/workflows/release.yml`. It validates package
versions with `scripts/check-release.mjs`, builds a universal app with the bundled
daemon, and attaches app/DMG/update artifacts to a **draft** GitHub release. A
maintainer reviews the draft and decides when to publish it.

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
Settings. These app distribution details are separate from the CLI packaging
issue; a locally passing build does not verify GitHub secrets, notarization, or
an end-to-end installed-app update.
