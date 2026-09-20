#!/usr/bin/python3
"""Install the opt-in prototype helper; does not change or restart Codex."""
from pathlib import Path
import shutil

from bridge import private_dir, registry_dir


if __name__ == '__main__':
    private_dir(registry_dir())
    destination = registry_dir() / 'prototype-v1'
    private_dir(destination)
    for name in ('bridge.py', 'wire.py', 'status.py', 'desktop_trial.py'):
        target = destination / name
        if target.is_symlink():
            raise ValueError('refusing to replace a symlink: ' + str(target))
        shutil.copyfile(Path(__file__).parent / name, target)
        target.chmod(0o700 if name != 'wire.py' else 0o600)
    print(destination)
