#!/usr/bin/env python3
"""Serve the production dashboard with fake IPC only, on localhost:8766."""
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(ROOT / 'tray' / 'ui'), **kwargs)
    def do_GET(self):
        if self.path in ('/', '/index.html'):
            body = (ROOT / 'tray' / 'ui' / 'index.html').read_text().replace('<script type="module"', '<script src="/fixture.js"></script><script type="module"').encode()
            self.send_response(200); self.send_header('Content-Type', 'text/html'); self.end_headers(); self.wfile.write(body)
        elif self.path == '/fixture.js':
            self.send_response(200); self.send_header('Content-Type', 'application/javascript'); self.end_headers(); self.wfile.write((ROOT / 'tests' / 'tray-fixture.js').read_bytes())
        else:
            super().do_GET()
ThreadingHTTPServer(('127.0.0.1', 8766), Handler).serve_forever()
