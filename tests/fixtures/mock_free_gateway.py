#!/usr/bin/env python3
"""A stand-in for the free model gateway (OpenAI-compatible, SSE streaming).
The first model is "rate-limited" (HTTP 429); the second replies with an
edit_file tool call, then a short answer once the tool result comes back."""
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        body = json.dumps({"data": [
            {"id": "kilo-auto/free", "name": "Auto", "supported_parameters": ["tools"]},
            {"id": "test/good:free", "name": "Good Free", "supported_parameters": ["tools"]},
            {"id": "paid/model", "name": "Paid", "supported_parameters": ["tools"]},
        ]}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def sse(self, chunks):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        self.wfile.write(b": KILO PROCESSING\n\n")
        for c in chunks:
            self.wfile.write(("data: " + json.dumps(c) + "\n\n").encode())
        self.wfile.write(b"data: [DONE]\n\n")

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if req["model"] == "kilo-auto/free":
            body = b'{"error":{"message":"Rate limit exceeded"}}'
            self.send_response(429)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        has_tool_result = any(m.get("role") == "tool" for m in req["messages"])
        if not has_tool_result:
            args = json.dumps({"path": "calc.py", "old_text": "return a - b", "new_text": "return a + b"})
            self.sse([
                {"choices": [{"delta": {"reasoning": "The operator is wrong."}}]},
                {"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1", "function": {"name": "edit_file", "arguments": args[:20]}}]}}]},
                {"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": args[20:]}}]}}]},
            ])
        else:
            self.sse([
                {"choices": [{"delta": {"content": "Fixed: `add()` now "}}]},
                {"choices": [{"delta": {"content": "adds."}}]},
                {"choices": [], "usage": {"total_tokens": 321}},
            ])

srv = HTTPServer(("127.0.0.1", 0), H)
print(srv.server_address[1], flush=True)
srv.serve_forever()
