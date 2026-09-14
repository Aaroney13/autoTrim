# CLI downloads and releases

The CLI release workflow prepares **draft** releases from existing `cli-vVERSION` tags. CLI releases use their own tag namespace so
they do not overwrite app release assets or notes.
A maintainer decides when to publish them. Publish CLI releases with “Set as
latest release” disabled (`gh release edit TAG --draft=false --latest=false`),
so the app updater’s latest-release endpoint continues to find app artifacts. Until a release is published on the
[Releases page](https://github.com/Aaroney13/autoTrim/releases), use the source
installation instructions in the README. No Rust toolchain is needed to use a
published binary.

| Platform | Target | Archive suffix | Verification |
| --- | --- | --- | --- |
| macOS Apple silicon | aarch64-apple-darwin | macos-aarch64.tar.gz | Native runner |
| macOS Intel | x86_64-apple-darwin | macos-x86_64.tar.gz | Native runner |
| Linux x86-64, glibc | x86_64-unknown-linux-gnu | linux-x86_64.tar.gz | Ubuntu 22.04 native runner |
| Windows x86-64 | x86_64-pc-windows-msvc | windows-x86_64.zip | Native runner |

Names start with `autotrim-VERSION-`. Linux builds require glibc 2.35 or newer;
ARM Linux, musl, and ARM Windows are not included. macOS minimum OS support is
determined by the Rust/sysinfo build requirements; these artifacts are tested
on the listed GitHub runners, not on every older macOS version. Windows may
require the Microsoft Visual C++ runtime already installed on most developer PCs.
Builds, unit tests, and `--version`/`--help` smoke checks do not establish that
notifications or monitoring work on a real Linux or Windows desktop.

## Download and verify

Download the archive for your machine and `SHA256SUMS` from the same release.
Compare the archive's SHA-256 digest to the matching line before extracting:

```sh
# macOS
shasum -a 256 autotrim-VERSION-macos-aarch64.tar.gz
# Linux
sha256sum autotrim-VERSION-linux-x86_64.tar.gz
```

```powershell
# Windows PowerShell
Get-FileHash .\autotrim-VERSION-windows-x86_64.zip -Algorithm SHA256
Expand-Archive .\autotrim-VERSION-windows-x86_64.zip -DestinationPath .
```

On macOS/Linux use `tar -xzf ARCHIVE.tar.gz`. Inside the extracted directory,
verify its files with `shasum -a 256 -c SHA256SUMS` on macOS or
`sha256sum -c SHA256SUMS` on Linux. Windows users can compare each listed file
with `Get-FileHash`. SHA-256 detects corruption; it is not code signing or an
independent proof of publisher identity.

## Install, upgrade, remove

On macOS/Linux, create `~/.local/bin`, copy the extracted `autotrim` there, and
ensure that directory is on `PATH`:

```sh
mkdir -p ~/.local/bin
install -m 755 ./autotrim ~/.local/bin/autotrim
export PATH="$HOME/.local/bin:$PATH"
autotrim --version
autotrim --help
```

Add the PATH line to your shell profile to retain it. On Windows, put the
extracted folder in a permanent location and add that folder to your **user**
Path in Environment Variables; reopen the terminal and run `autotrim --version`.
Retain the documentation and third-party notices with your installation.

On macOS, optionally run `autotrim service install` to start the daemon and
load it at login. The downloaded CLI does not install the Tauri app.

To upgrade, first `autotrim service uninstall` on macOS if installed, or stop a
foreground daemon with Ctrl-C. Verify the new archive, replace the binary, and
reinstall the macOS service if wanted. Windows cannot replace a running binary.
To remove it, uninstall the service first, delete the binary/extracted folder,
and remove any PATH entry you added. Configuration, history, and recovery logs
stay in the data directory described in README; remove that directory separately
only if you no longer need it.

These binaries are unsigned and macOS downloads are not notarized. Gatekeeper,
SmartScreen, or organization policy may block them. Verify their origin and
checksums and follow your platform's normal per-application approval process;
do not disable system security globally. App bundles, signing/notarization,
updaters, and package-manager publishing have separate distribution requirements.

## Maintainer workflow

Create a version tag only after Cargo.toml and Cargo.lock are ready and the
changes are merged. The workflow checks out `refs/tags/cli-vVERSION`, rejects a
package-version mismatch, and builds with `--locked`. Manual dispatch accepts an
existing tag for recovery. Each runner packages notices and an internal manifest,
extracts the archive, verifies every file, and runs the extracted CLI.

No draft/upload job runs unless every target succeeds. The upload job marks its
draft incomplete before replacing assets and marks it verified only after all
uploads succeed. Failed uploads therefore remain explicitly incomplete drafts.
Re-running the same immutable tag replaces draft assets and checksums; it refuses
to alter a published release. A prior successful draft remains valid if a later
build fails before upload. Never move a release tag; publish a new version for
changed source. Binaries need not be bit-for-bit identical across reruns with a
new stable compiler, so always download the checksum from the same release run.

The repository currently specifies no project license. Release packaging includes
an explicit notice of that fact and available dependency licenses; selecting a
project license remains a maintainer decision.
