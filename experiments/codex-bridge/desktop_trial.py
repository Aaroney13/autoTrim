#!/usr/bin/python3
"""One desktop restart with bounded verification and automatic launch rollback.

Run only after the user's tasks have finished. This script performs no cleanup.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time

from bridge import private_dir, registry_dir
from wire import Client

APP = "/Applications/ChatGPT.app"


def running():
    result = subprocess.run(['/bin/ps', '-axo', 'comm='], capture_output=True, text=True, check=True)
    return any(line.strip().startswith(APP + '/Contents/MacOS/') for line in result.stdout.splitlines())


def launch(bridge):
    args = ['/usr/bin/open', '-a', APP]
    # This applies only to this launch; no global launchctl or app config changes.
    if bridge:
        args += ['--env', 'CODEX_CLI_PATH=' + str(Path(__file__).parent / 'bridge.py')]
    subprocess.run(args, check=True, timeout=15)


def quit_app():
    subprocess.run(['/usr/bin/osascript', '-e', 'tell application "' + APP + '" to quit'],
                   timeout=30, check=True, capture_output=True)
    deadline = time.monotonic() + 30
    while running():
        if time.monotonic() > deadline:
            raise RuntimeError('Codex did not quit; no force termination was attempted')
        time.sleep(.5)


def save(value):
    destination = registry_dir() / 'last-trial-result.txt'
    temp = destination.with_suffix('.tmp')
    with open(temp, 'w', opener=lambda path, flags: os.open(path, flags, 0o600)) as stream:
        json.dump(value, stream, indent=2)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temp, destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--restart', action='store_true', help='Quit the running app before the trial')
    parser.add_argument('--restore', action='store_true', help='Quit and reopen normally without the bridge')
    parser.add_argument('--delay', type=int, default=0, choices=range(0, 61), metavar='SECONDS')
    args = parser.parse_args()
    private_dir(registry_dir())
    save({'trial_status': 'pending', 'requested_at': int(time.time()), 'cleanup_enabled': False})
    if args.delay:
        time.sleep(args.delay)
    if running():
        if not (args.restart or args.restore):
            parser.error('Quit Codex first or explicitly use --restart after your tasks finish')
        quit_app()
    if args.restore:
        launch(False)
        print('Opened Codex normally, without setting a bridge override.')
        return
    started = int(time.time())
    launch(True)
    deadline = time.monotonic() + 60
    result = {'started_at': started, 'desktop_connected': False, 'cleanup_enabled': False}
    while time.monotonic() < deadline:
        for path in registry_dir().glob('*.json'):
            client = None
            try:
                manifest = json.loads(path.read_text())
                if manifest.get('created_at', 0) < started or not manifest.get('desktop_initialized_at'):
                    continue
                client = Client(manifest['socket'])
                loaded = client.request('thread/loaded/list', {})
                result.update(trial_status='connected', desktop_connected=True, backend_pid=manifest['backend_pid'],
                              bridge_pid=manifest['bridge_pid'], socket=manifest['socket'],
                              loaded_task_count=len(loaded['data']), checked_at=int(time.time()))
                save(result)
                print('Desktop and AutoTrim both connected to the same backend. Cleanup remains off.')
                return
            except (OSError, ValueError, RuntimeError, EOFError):
                continue
            finally:
                if client:
                    client.wire.close()
        time.sleep(1)
    result['error'] = 'No initialized desktop bridge could be verified within 60 seconds'
    result['trial_status'] = 'failed'
    # The user authorized one trial. Restore ordinary startup if it fails.
    try:
        if running():
            quit_app()
        launch(False)
        result['normal_launch_restored'] = True
    except Exception as error:
        result['rollback_error'] = str(error)
    save(result)
    raise RuntimeError(result['error'])


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print('AutoTrim desktop trial: ' + str(error), file=sys.stderr)
        sys.exit(1)
