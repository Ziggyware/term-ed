#!/usr/bin/env python3
"""Mock free gateway: 503 on chat completions, healthy /models.

Enough of the OpenAI surface for the built-in agent to discover a model and
then fail with HTTP 503, which is the case the local ONNX fallback covers.
Prints the port it listens on, then serves until killed.
"""

import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _json(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = self.path.split("?", 1)[0].rstrip("/")
        if path.endswith("/models"):
            self._json(200, {
                "object": "list",
                "data": [
                    {"id": "unavailable/model:free", "object": "model"},
                ],
            })
        else:
            self._json(404, {"error": {"message": "not found"}})

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        if length:
            self.rfile.read(length)
        path = self.path.split("?", 1)[0].rstrip("/")
        if path.endswith("/chat/completions"):
            self._json(503, {
                "error": {
                    "message": "Service unavailable: no upstream capacity",
                    "type": "server_error",
                }
            })
        else:
            self._json(404, {"error": {"message": "not found"}})


if __name__ == "__main__":
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(server.server_address[1], flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        sys.exit(0)
