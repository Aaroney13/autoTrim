"""Small, bounded WebSocket client for Codex's private Unix transport.

Prototype only; no TCP listener, external dependencies, or model requests.
"""
import base64
import hashlib
import json
import os
import socket
import stat
import struct
import threading

MAX_MESSAGE = 64 * 1024 * 1024


class Wire:
    def __init__(self, sock):
        self.sock = sock
        self.lock = threading.Lock()

    @classmethod
    def connect(cls, path):
        parent = os.stat(os.path.dirname(path), follow_symlinks=False)
        endpoint = os.stat(path, follow_symlinks=False)
        if (not stat.S_ISDIR(parent.st_mode) or parent.st_uid != os.getuid()
                or parent.st_mode & 0o077 or not stat.S_ISSOCK(endpoint.st_mode)
                or endpoint.st_uid != os.getuid()):
            raise ValueError("socket must belong to this user in a private directory")
        sock = socket.socket(socket.AF_UNIX)
        try:
            sock.settimeout(10)
            sock.connect(path)
            wire = cls(sock)
            key = base64.b64encode(os.urandom(16)).decode()
            sock.sendall(("GET /rpc HTTP/1.1\r\nHost: localhost\r\n"
                          "Upgrade: websocket\r\nConnection: Upgrade\r\n"
                          "Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: "
                          + key + "\r\n\r\n").encode())
            header = bytearray()
            while not header.endswith(b"\r\n\r\n"):
                if len(header) >= 16384:
                    raise ValueError("oversized WebSocket handshake")
                header.extend(wire.exact(1))
            lines = bytes(header).decode("ascii").split("\r\n")
            fields = dict(line.lower().split(":", 1) for line in lines[1:] if ":" in line)
            accept = base64.b64encode(hashlib.sha1(
                (key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
            # Header names are case-insensitive; the base64 value is not.
            original = dict((k.lower(), v.strip()) for k, v in
                            (line.split(":", 1) for line in lines[1:] if ":" in line))
            if (lines[0].split()[1] != "101"
                    or fields.get("upgrade", "").strip() != "websocket"
                    or "upgrade" not in fields.get("connection", "")
                    or original.get("sec-websocket-accept") != accept):
                raise ValueError("invalid WebSocket handshake")
            sock.settimeout(None)
            return wire
        except BaseException:
            sock.close()
            raise

    def exact(self, size):
        data = bytearray()
        while len(data) < size:
            chunk = self.sock.recv(size - len(data))
            if not chunk:
                raise EOFError("backend connection closed")
            data.extend(chunk)
        return bytes(data)

    def send_frame(self, data, opcode=1):
        if len(data) > MAX_MESSAGE:
            raise ValueError("WebSocket message exceeds 64 MiB")
        size = len(data)
        header = bytes([0x80 | opcode, 0x80 | (size if size < 126 else 126 if size < 65536 else 127)])
        if size >= 126:
            header += struct.pack("!H" if size < 65536 else "!Q", size)
        mask = os.urandom(4)
        payload = bytearray(data)
        # Avoid a Python loop for every byte of a large transcript.
        for i in range(4):
            payload[i::4] = payload[i::4].translate(bytes(x ^ mask[i] for x in range(256)))
        with self.lock:
            self.sock.sendall(header + mask + payload)

    def send(self, value):
        self.send_frame(json.dumps(value, separators=(",", ":")).encode())

    def receive(self):
        message = bytearray()
        fragmented = False
        while True:
            first, second = self.exact(2)
            final, opcode, size = bool(first & 0x80), first & 15, second & 127
            if first & 0x70 or second & 0x80:
                raise ValueError("unexpected compressed or masked server frame")
            if size == 126:
                size = struct.unpack("!H", self.exact(2))[0]
            elif size == 127:
                size = struct.unpack("!Q", self.exact(8))[0]
            if opcode >= 8:
                if not final or size > 125:
                    raise ValueError("invalid control frame")
                payload = self.exact(size)
                if opcode == 8:
                    if len(payload) == 1:
                        raise ValueError("invalid close frame")
                    if payload and struct.unpack("!H", payload[:2])[0] not in (1000, 1001):
                        raise ValueError("backend closed WebSocket abnormally")
                    raise EOFError("backend closed WebSocket")
                if opcode == 9:
                    self.send_frame(payload, 10)
                elif opcode != 10:
                    raise ValueError("unknown control frame")
                continue
            if size + len(message) > MAX_MESSAGE:
                raise ValueError("WebSocket message exceeds 64 MiB")
            if opcode != (0 if fragmented else 1):
                raise ValueError("expected a text message or continuation")
            message.extend(self.exact(size))
            if final:
                return bytes(message)
            fragmented = True

    def close(self):
        try:
            self.sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        self.sock.close()


class Client:
    """Read-only callers initialize their own independent connection."""
    def __init__(self, path, name="autotrim-bridge-inspect"):
        self.wire = Wire.connect(path)
        self.number = 0
        self.wire.sock.settimeout(10)
        self.request("initialize", {"clientInfo": {"name": name, "version": "0.1"},
                                    "capabilities": {"experimentalApi": True}})
        self.wire.send({"method": "initialized"})

    def request(self, method, params):
        self.number += 1
        request_id = self.number
        self.wire.send({"id": request_id, "method": method, "params": params})
        # Ignore notifications, without retaining private task contents.
        while True:
            value = json.loads(self.wire.receive())
            if value.get("id") == request_id and "method" not in value:
                if "error" in value:
                    raise RuntimeError(value["error"])
                return value["result"]
