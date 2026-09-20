# Proposed licensing for AutoTrim

Status: research and implementation proposal, not a license grant. No project
license has been applied by this document.

## Recommendation

Use MPL-2.0 for the public desktop app, CLI, and daemon if distributed changes
to the existing core should remain open. Keep the independently written AI
server in a separate private repository under proprietary terms. MPL permits
separate files containing no covered code to use other licenses; distributed
covered files and modifications remain subject to MPL. See the
[license, sections 1.10 and 3](https://www.mozilla.org/en-US/MPL/2.0/).

Apache-2.0 is the alternative if closed-source forks of the client are acceptable.
It permits proprietary derivatives and includes an express contributor patent
grant. See the [Apache license](https://www.apache.org/licenses/LICENSE-2.0.html).

Either option allows commercial use. An open-source license cannot prohibit
competitors or commercial use while meeting the
[Open Source Definition](https://opensource.org/osd). Licensing the client
should not be presented as licensing access to the hosted AI service.

## Product boundary

Proposed arrangement:

| Component | Location | Access |
| --- | --- | --- |
| Local monitoring, manual cleanup, settings, and UI | Public AutoTrim repository | Free; no account required |
| Client code for account connection and AI requests | Public AutoTrim repository | Source remains available; service requests require authorization |
| AI processing, provider credentials, subscription entitlements, and usage limits | Separate private server repository | Proprietary service; authenticated paid access |

The server should check the account's entitlement on every protected request.
Changing or removing a client paywall must not grant server access. This follows
[OWASP's server-side authorization guidance](https://cheatsheetseries.owasp.org/cheatsheets/Authorization_Cheat_Sheet.html).
Keep provider secrets on the server. Make access revocation disable hosted
features while leaving free local functionality usable.

Keep server implementation out of this repository and its release assets. If
code is shared, document which license applies; moving covered code into another
repository does not remove its license obligations. A separate repository is a
maintenance boundary, not a way to change code ownership or licensing.

## Repository findings

Checked on September 19, 2026:

- No root project LICENSE file and no license field in either Cargo package.
- `docs/cli-releases.md` explicitly records the absence of a project license.
- `scripts/package-release.py` writes a no-project-license notice and collects
  third-party notices. It does not currently copy a root project license.
- Git history includes Aaroney13, Aaron Epstein, and Aleem Virani. Author names
  alone do not establish ownership or permission to license contributions.
- Current privacy documentation describes local monitoring without AI uploads.

Without a license, public visibility does not grant the usual open-source reuse
rights. GitHub separately permits viewing and forking public repositories under
its platform terms. See [GitHub's licensing guidance](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/licensing-a-repository).

## Applying the selected license

1. Establish the copyright holders and rights to license existing contributions.
   Record an explicit policy for future contributions under the selected license.
2. Add the unmodified standard license text as root `LICENSE`. For MPL, attach
   its notice to covered source files, using `SPDX-License-Identifier: MPL-2.0`
   where appropriate, and preserve existing third-party notices. See
   [Mozilla's application instructions](https://www.mozilla.org/en-US/MPL/2.0/FAQ/#q4-i-want-to-use-the-mozilla-public-license-for-software-that-i-have-written-what-do-i-have-to-do).
3. Set `license = "MPL-2.0"` in the package sections of both `Cargo.toml` and
   `tray/Cargo.toml`, or `Apache-2.0` if that option is selected. Cargo accepts
   [SPDX license identifiers](https://doc.rust-lang.org/cargo/reference/manifest.html#the-license-and-license-file-fields).
4. Add a README licensing statement defining the public-client/private-service
   boundary. Replace the no-license statements in CLI packaging and release docs.
5. Include LICENSE and required third-party notices in CLI archives and desktop
   bundles. For MPL builds, provide a clear route to the corresponding covered
   source, preferably the exact release tag. Verify the packaged artifacts.
6. Before enabling hosted AI, publish service terms and update the privacy
   disclosure and UI to explain what data is sent, the processors involved,
   retention, and deletion. Preserve the local-only free path.

Suggested README wording after MPL adoption:

> AutoTrim's desktop app, CLI, and daemon are licensed under MPL-2.0. Local
> features are free to use without an account. Optional hosted AI features use
> a separate proprietary service and require an account and eligible plan.
> The client license does not grant access to that service.

This proposal is general licensing information, not a legal opinion. An IP
lawyer can confirm contribution rights and the final client/service boundary.
