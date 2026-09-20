# Individual Codex desktop task cleanup

Research date: September 14, 2026. Installed desktop: 26.908.40834.
Bundled backend: Codex Desktop 0.154.0-alpha.6.2.

## Finding

A native AutoTrim integration is technically feasible. The backend accepts an
archive request from a second local client, unloads the selected task, and informs
the original client. It does not require a model call. The desktop connection
still needs an integration prototype: this research did not reconfigure or
restart the user's app, and did not archive any real tasks.

## Verified experiment

Started the installed backend with a private Unix socket and a temporary Codex
home. Connected two independent WebSocket clients. The first simulated the
desktop and resumed two synthetic persisted tasks; the second simulated AutoTrim.

1. Both tasks appeared in `thread/loaded/list`.
2. AutoTrim's client called `thread/unsubscribe` for the first client's task.
   The result was `notSubscribed`, and the task stayed loaded.
3. AutoTrim's client called `thread/archive` for that task.
4. The first client received `thread/archived`. Its next loaded-task listing
   contained only the other task. The backend remained running.
5. `thread/unarchive` restored the archived task successfully.

No turn was started, no model request was made, and the scratch backend was
terminated afterward. This verifies backend interoperability, not desktop UI,
plugins, approvals, scheduled-task behavior, or a measured reduction in RAM.

Experiment script: `/tmp/autotrim-codex-two-client-test.py`.
Result: `/tmp/autotrim-two-clients-okcevvq7/result.json`.

## Connection options

| Route | Evidence | Assessment |
| --- | --- | --- |
| Shared managed daemon | Desktop retains `CODEX_APP_SERVER_USE_LOCAL_DAEMON=1`, but its branch requires no config overrides. This build always supplies an app-tools override, even when its value is false. Other startup gates also apply. | Setting that variable alone will not work in this build. |
| Explicit WebSocket backend | Desktop checks `CODEX_APP_SERVER_WS_URL` before its private-stdio transport. | A plausible experimental route. A separately launched backend must receive the desktop's configuration and app-tool environment correctly. Not tested with the real desktop. |
| AutoTrim launcher bridge | Desktop accepts a CLI executable override through `CODEX_CLI_PATH`; its launch path appends the app's configuration overrides. Backend Unix-socket control was verified with two clients. | Recommended prototype: preserve the desktop launch arguments/environment, start its bundled backend on a private socket, and translate the desktop's JSONL stdio connection to WebSocket. AutoTrim connects independently to that same backend. |
| Accessibility-driven Archive | The app already exposes Archive in its UI. | A potential macOS fallback, not tested here; requires permission and is vulnerable to UI changes and focus races. |

The launcher bridge would be a new integration component, not a feature Codex
officially guarantees. It needs explicit setup and a desktop restart before it
can control tasks. It cannot attach retroactively to an existing private stdio
backend. It should preserve native tool routing, config flags, stderr, request
IDs, approvals, notifications, shutdown semantics and exit codes. Non-app-server
CLI invocations must delegate unchanged to the real bundled binary.

Use a private Unix socket rather than publishing a remote listener. Keep the
bridge independent from AutoTrim's cleanup policy: a monitor restart should not
terminate Codex. Test installation, removal and desktop updates before shipping.

## Cleanup behavior and limits

Use the explicit name **Archive idle Codex tasks**: archiving changes task
visibility and can cascade to descendants. Protect active turns, pending
approvals, background work, goals, pinned tasks, scheduled follow-ups and relevant
descendants. Recheck activity just before execution and save recovery details.
The protocol does not establish an atomic archive-if-still-idle precondition.
Desktop handling of archive notifications and automation side effects needs
end-to-end verification before automatic cleanup is enabled.

`thread/unsubscribe` only removes the caller's subscription. It is not a general
command to force another client's task out of memory. The documented idle unload
requires no subscribers and no activity for 30 minutes. A new AutoTrim connection
cannot satisfy that while the desktop is still subscribed.

No per-task RAM measurement was found. AutoTrim can verify task disappearance
from the loaded list and show shared backend RAM separately.

## Sources

- [OpenAI App Server documentation](https://learn.chatgpt.com/docs/app-server):
  transport framing, loaded-task listing, archive/unarchive and unsubscribe.
- [Shared-daemon setup report, issue 31991](https://github.com/openai/codex/issues/31991):
  a user-reported previously working shared desktop/backend configuration.
- [Desktop daemon regression report, issue 41014](https://github.com/openai/codex/issues/41014):
  the reported configuration-override startup conflict. This is an issue report,
  not an official support commitment; local source inspection corroborated the
  relevant condition in the installed build.
- Installed desktop bundle: inspected transport selection and CLI launch code
  from `app.asar`, without modifying it. Relevant functions in this version's
  extracted source were `SU.connect`, `DH`, `jU`, `NU`, and `lie`.

The earlier separate Codex cleanup heartbeat remains paused. It is not the native
integration proposed here.
