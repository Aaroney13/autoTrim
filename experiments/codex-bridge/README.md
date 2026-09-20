# Codex bridge prototype (macOS)

An opt-in transport experiment. It does not enable automatic cleanup, make
model calls, edit the Codex app, or change its backend code. The ordinary
AutoTrim daemon and app do not depend on these Python files.

The desktop launches `bridge.py` using a per-launch `CODEX_CLI_PATH` override.
The bridge forwards the app's arguments and environment to its installed Codex
binary, replacing only stdio transport with a private Unix socket. It translates
the desktop's JSONL connection to WebSocket messages. A separate AutoTrim client
can reach the same backend. No TCP listener is opened.

The backend belongs to the desktop bridge, not to the AutoTrim daemon. Desktop
stdin EOF, a termination signal, or backend exit shuts the bridge down and removes
that run's socket and registry entry. The helper does not independently start
or resume user tasks. Commands other than stdio `app-server` delegate unchanged
to the real CLI. Protocol messages and credentials are not logged.

## Test

Requires macOS's `/usr/bin/python3` (3.9+) and the installed bundled Codex binary.
The Python interpreter currently comes with Apple's developer tools; this is
not yet a packaged dependency for distributing AutoTrim to other users.

```sh
python3 -m unittest discover -s experiments/codex-bridge -p 'test_*.py' -v
AUTOTRIM_TEST_CODEX_CLI=/Applications/ChatGPT.app/Contents/Resources/codex \
  python3 -m unittest discover -s experiments/codex-bridge -p 'test_*.py' -v
```

Real-backend tests use temporary Codex and SQLite directories and synthetic
rollouts. They do not start turns or touch real tasks. Checks cover independent
client archiving, restoration, notification delivery to the desktop connection,
read-only inspection, CLI argument delegation, fragmented messages, ping/pong,
large-frame masking, frame bounds, backend failure, stdin EOF and SIGTERM.

## Install and try

```sh
python3 experiments/codex-bridge/install.py
```

The helper is copied to `~/Library/Application Support/autotrim/codex-bridge/prototype-v1`.
After tasks finish, fully quit Codex and run:

```sh
python3 "$HOME/Library/Application Support/autotrim/codex-bridge/prototype-v1/desktop_trial.py"
```

Or explicitly pass `--restart` to request a normal app quit before the trial.
No global `launchctl` environment or app configuration is changed. The one-launch
override takes effect only when a new desktop process starts. The trial checks
that the desktop completed initialization through the bridge and that a second
client can list loaded tasks. It restores normal app startup if that cannot be
verified within 60 seconds. It never force-quits the desktop app.

Read `codex-bridge/last-trial-result.txt` for the startup result. For live,
read-only status:

```sh
python3 "$HOME/Library/Application Support/autotrim/codex-bridge/prototype-v1/status.py"
```

To quit the trial app and reopen normally after tasks finish:

```sh
python3 "$HOME/Library/Application Support/autotrim/codex-bridge/prototype-v1/desktop_trial.py" --restore
```

Remove the prototype directory after normal startup to uninstall. The private
registry contains PID/socket metadata only. Concurrent desktop backends receive
different sockets and registry files. Socket access is limited to the same user.

## Limits

This is a prototype, not a shipped native cleanup setting. Native approval and
plugin workflows, update compatibility and long-running load still need desktop
testing. Each message is capped at 64 MiB. A real archive action would also need
AutoTrim's shared recovery journal, idle revalidation, and protection for scheduled
tasks and descendants before being exposed as a UI control. Per-task RAM is
not available from this protocol. The previous Codex heartbeat remains paused.
