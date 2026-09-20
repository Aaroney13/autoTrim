#!/usr/bin/python3
"""Experimental CODEX_CLI_PATH launcher. Never starts turns or cleanup."""
import json
import os
from pathlib import Path
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time

from wire import MAX_MESSAGE, Wire

REAL_CLI = "/Applications/ChatGPT.app/Contents/Resources/codex"


def bridge_arguments(args):
    """Intercept only a stdio app-server launch; preserve other CLI commands."""
    takes_value = {"-c", "--config", "--enable", "--disable", "--code-mode-host"}
    index = 0
    while index < len(args):
        arg = args[index]
        if arg in takes_value:
            index += 2
        elif arg.startswith("-"):
            if arg in ("--help", "-h", "--version", "-V"):
                return None
            index += 1
        else:
            break
    if index >= len(args) or args[index] != "app-server":
        return None
    out = args[:index + 1]
    index += 1
    while index < len(args):
        arg = args[index]
        if arg in ("--help", "-h", "--version", "-V") or not arg.startswith("-"):
            return None
        if arg == "--stdio":
            index += 1
            continue
        if arg.startswith("--listen="):
            if arg != "--listen=stdio://":
                return None
            index += 1
            continue
        if arg == "--listen":
            if args[index + 1:index + 2] != ["stdio://"]:
                return None
            index += 2
            continue
        out.append(arg)
        if arg in takes_value or arg in {"--ws-auth", "--ws-token-file", "--ws-token-sha256",
                                        "--ws-shared-secret-file", "--ws-issuer", "--ws-audience",
                                        "--ws-max-clock-skew-seconds"}:
            if index + 1 >= len(args):
                return None
            out.append(args[index + 1])
            index += 1
        index += 1
    return out


def registry_dir():
    override = os.environ.get("AUTOTRIM_CODEX_BRIDGE_DIR")
    return Path(override) if override else Path.home() / "Library/Application Support/autotrim/codex-bridge"


def private_dir(path):
    path.mkdir(parents=True, mode=0o700, exist_ok=True)
    meta = path.lstat()
    if not stat.S_ISDIR(meta.st_mode) or meta.st_uid != os.getuid() or meta.st_mode & 0o077:
        raise ValueError("bridge directory must be a private directory owned by this user")


def run(args, real_cli):
    directory = registry_dir()
    private_dir(directory)
    # Unix socket paths on macOS must fit in 104 bytes; the data directory won't.
    scratch = Path(tempfile.mkdtemp(prefix="autotrim-codex-", dir="/tmp"))
    socket_path = str(scratch / "rpc.sock")
    manifest = directory / (str(os.getpid()) + ".json")
    child = None
    wire = None
    manifest_created = False
    stopped = threading.Event()
    failure = []
    interrupted = []
    initialize_id = []

    def stop():
        stopped.set()
        if wire is not None:
            wire.close()

    def on_signal(number, _frame):
        interrupted.append(number)
        stop()

    for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(number, on_signal)
    try:
        child = subprocess.Popen([real_cli] + args + ["--listen", "unix://" + socket_path],
                                 stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL)
        deadline = time.monotonic() + 15
        while not Path(socket_path).exists():
            code = child.poll()
            if code is not None:
                return code if code >= 0 else 128 - code
            if stopped.wait(.05):
                return 128 + interrupted[0] if interrupted else 1
            if time.monotonic() > deadline:
                raise TimeoutError("Codex socket did not become ready within 15 seconds")
        wire = Wire.connect(socket_path)
        data = {"schema_version": 1, "bridge_pid": os.getpid(), "backend_pid": child.pid,
                "socket": socket_path, "real_cli": real_cli,
                "codex_home": os.environ.get("CODEX_HOME", str(Path.home() / ".codex")),
                "created_at": int(time.time())}
        with open(manifest, "x", opener=lambda path, flags: os.open(path, flags, 0o600)) as stream:
            manifest_created = True
            json.dump(data, stream)
            stream.flush()
            os.fsync(stream.fileno())

        def send_input():
            pending = bytearray()
            try:
                while not stopped.is_set():
                    # Raw fd reads avoid a daemon thread holding Python's
                    # buffered-stdin lock during signal/child-exit shutdown.
                    chunk = os.read(sys.stdin.fileno(), 65536)
                    if not chunk:
                        if pending:
                            raise ValueError("incomplete desktop JSONL message")
                        break
                    pending.extend(chunk)
                    while b"\n" in pending:
                        index = pending.index(b"\n")
                        if index > MAX_MESSAGE:
                            raise ValueError("desktop message exceeds 64 MiB")
                        payload = bytes(pending[:index]).rstrip(b"\r")
                        del pending[:index + 1]
                        if payload:
                            request = json.loads(payload)
                            if request.get("method") == "initialize":
                                initialize_id[:] = [request.get("id")]
                            wire.send_frame(payload)
                    if len(pending) > MAX_MESSAGE:
                        raise ValueError("desktop message exceeds 64 MiB")
            except Exception as error:
                if not stopped.is_set():
                    failure.append(error)
            finally:
                stop()

        def watch_child():
            child.wait()
            stop()

        threading.Thread(target=send_input, daemon=True).start()
        threading.Thread(target=watch_child, daemon=True).start()
        try:
            while not stopped.is_set():
                raw = wire.receive()
                # The socket may pretty-print JSON; stdio requires one line.
                value = json.loads(raw)
                encoded = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()
                sys.stdout.buffer.write(encoded + b"\n")
                sys.stdout.buffer.flush()
                if initialize_id and value.get("id") == initialize_id[0] and "result" in value:
                    data["desktop_initialized_at"] = int(time.time())
                    temp_manifest = manifest.with_suffix(".tmp")
                    try:
                        with open(temp_manifest, "x", opener=lambda path, flags: os.open(path, flags, 0o600)) as stream:
                            json.dump(data, stream)
                            stream.flush()
                            os.fsync(stream.fileno())
                        os.replace(temp_manifest, manifest)
                    except OSError:
                        print("AutoTrim Codex bridge: registry update failed", file=sys.stderr)
                    finally:
                        temp_manifest.unlink(missing_ok=True)
                    initialize_id.clear()
        except Exception as error:
            if not stopped.is_set():
                failure.append(error)
        stop()
        if interrupted:
            return 128 + interrupted[0]
        code = child.poll()
        if code is None and failure:
            # Socket EOF can precede waitpid observing a crashed child.
            try:
                code = child.wait(timeout=1)
            except subprocess.TimeoutExpired:
                pass
        if code is not None:
            return code if code >= 0 else 128 - code
        if failure:
            # Do not log protocol payloads or credentials.
            print("AutoTrim Codex bridge: transport failed (" + type(failure[0]).__name__ + ")", file=sys.stderr)
            return 1
        return 0
    finally:
        stop()
        if child is not None and child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        # Never unlink another run's socket or registry.
        if manifest_created:
            manifest.unlink(missing_ok=True)
        Path(socket_path).unlink(missing_ok=True)
        scratch.rmdir()


def main():
    real_cli = os.environ.get("AUTOTRIM_CODEX_REAL_CLI", REAL_CLI)
    if Path(real_cli).resolve() == Path(__file__).resolve():
        raise ValueError("real CLI cannot be the bridge itself")
    args = bridge_arguments(sys.argv[1:])
    if args is None:
        os.execv(real_cli, [real_cli] + sys.argv[1:])
    return run(args, real_cli)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print("AutoTrim Codex bridge: " + str(error), file=sys.stderr)
        sys.exit(1)
