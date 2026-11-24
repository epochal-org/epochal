"""Serves an HTTP API on this machine to a program in the box, through the relay.

The box has no network, yet its script reads ``http://127.0.0.1:8787`` and gets an answer from
the server below. The key the server holds stays here: the box only sees what the server sends.

    export EPOCHAL_API_KEY=...
    python forward.py
"""

from __future__ import annotations

import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from epochal.sandbox import Sandbox

API_KEY = "sk-stays-on-this-machine"

IN_THE_BOX = """
import json, sys, urllib.request
with urllib.request.urlopen(f"http://127.0.0.1:{sys.argv[1]}/quote?ticker=NVDA") as r:
    print(json.load(r))
"""


class Quotes(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        # A real server would call upstream with API_KEY here; only the answer goes back.
        body = json.dumps({"ticker": "NVDA", "close": 124.92})
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body.encode())

    def send_response(self, code: int, message: str | None = None) -> None:
        # The stock one adds a Date header, which would tell the box today's date.
        self.log_request(code)
        self.send_response_only(code, message)


def main(box_port: int = 8787) -> str:
    with ThreadingHTTPServer(("127.0.0.1", 0), Quotes) as server:
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            with (
                Sandbox.time_portal("2025-03-12").with_relay().build() as sandbox,
                sandbox.forward(box_port, to=("127.0.0.1", server.server_port)),
            ):
                script = sandbox.command(["python3", "-c", IN_THE_BOX, str(box_port)])
                return script.output(check=True).text
        finally:
            server.shutdown()


if __name__ == "__main__":
    print(main(*map(int, sys.argv[1:])), end="")
