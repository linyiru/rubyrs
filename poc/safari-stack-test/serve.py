#!/usr/bin/env python3
"""Static file server + result-capture endpoint for the Safari stack-overflow PoC.

GET /*     -> static files from cwd (with correct MIME for .wasm).
POST /report -> body is JSON; appended to results.jsonl with a timestamp.

Usage:  python3 serve.py [PORT]   (default 8765)
"""
import http.server
import json
import os
import socketserver
import sys
import time

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
LOG = os.path.join(os.path.dirname(os.path.abspath(__file__)), "results.jsonl")


class Handler(http.server.SimpleHTTPRequestHandler):
    extensions_map = {
        **http.server.SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".html": "text/html; charset=utf-8",
        ".js":   "text/javascript; charset=utf-8",
        ".rb":   "text/plain; charset=utf-8",
    }

    def end_headers(self):
        # CORS so the page can POST to itself without preflight grief if
        # someone reloads from a different origin during testing.
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type")
        super().end_headers()

    def do_OPTIONS(self):
        self.send_response(204)
        self.end_headers()

    def do_POST(self):
        if self.path != "/report":
            self.send_error(404)
            return
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        try:
            payload = json.loads(body.decode("utf-8"))
        except Exception as e:
            self.send_error(400, f"bad json: {e}")
            return
        payload["ts"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        with open(LOG, "a") as fh:
            fh.write(json.dumps(payload, ensure_ascii=False) + "\n")
        # Mirror to stdout so we see results live in the terminal.
        print(f"[report] {payload.get('browser','?')} / {payload.get('which','?')}: "
              f"{payload.get('lines',0)} lines, exit={payload.get('exit','?')}")
        sys.stdout.flush()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"ok":true}')


class ReusableTCPServer(socketserver.TCPServer):
    allow_reuse_address = True


if __name__ == "__main__":
    os.chdir(os.path.dirname(os.path.abspath(__file__)))
    with ReusableTCPServer(("127.0.0.1", PORT), Handler) as httpd:
        print(f"serving http://localhost:{PORT}/  (log -> {LOG})")
        httpd.serve_forever()
