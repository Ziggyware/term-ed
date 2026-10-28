#!/usr/bin/env python3
"""Local authenticated, router-shaped completion fixture; no real credentials.

Exercise providers which ignore stream=true, return object-form arguments, omit
call IDs, and later send an SSE finish event without a delta.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        try:
            assert self.path == "/v1/chat/completions"
            assert self.headers.get("Authorization") == "Bearer test-only-key+plus"
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            assert request["model"] == "free:code"
            assert request["stream"] is True
            names = {tool["function"]["name"] for tool in request["tools"]}
            assert {"read_file", "edit_file"} <= names
            messages = request["messages"]
            results = [m for m in messages if m["role"] == "tool"]
            assistants = [m for m in messages if m["role"] == "assistant" and m.get("tool_calls")]
            if not results:
                body = {
                    "choices": [{
                        "message": {"role": "assistant", "content": None, "tool_calls": [{
                            "type": "function",
                            "function": {"name": "read_file", "arguments": {"path": "calc.py"}},
                        }]},
                        "finish_reason": "tool_calls",
                    }],
                }
                data = json.dumps(body).encode()
                content_type = "application/json; charset=utf-8"
            elif len(results) == 1:
                first = assistants[0]["tool_calls"][0]
                assert first["id"] and results[0]["tool_call_id"] == first["id"]
                assert json.loads(first["function"]["arguments"]) == {"path": "calc.py"}
                assert "return a - b" in results[0]["content"]
                event = {"choices": [{"delta": {"tool_calls": [{
                    "type": "function",
                    "function": {"name": "edit_file", "arguments": {
                        "path": "calc.py", "old_text": "return a - b", "new_text": "return a + b",
                    }},
                }]}, "finish_reason": None}]}
                # Finish reasons need not be accompanied by a delta or [DONE].
                finish = {"choices": [{"finish_reason": "tool_calls"}]}
                data = ("data: " + json.dumps(event) + "\n\n" +
                        "data: " + json.dumps(finish) + "\n\n").encode()
                content_type = "text/event-stream"
            else:
                assert len(results) == 2
                first = assistants[0]["tool_calls"][0]["id"]
                second = assistants[1]["tool_calls"][0]["id"]
                assert first and second and first != second
                assert results[1]["tool_call_id"] == second
                assert "Edited calc.py" in results[1]["content"]
                event = {"choices": [{"delta": {"content": "The addition is fixed."}, "finish_reason": "stop"}]}
                data = ("data: " + json.dumps(event) + "\n\ndata: [DONE]\n\n").encode()
                content_type = "text/event-stream"
            self.send_response(200)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        except Exception as exc:
            data = json.dumps({"error": {"message": "fixture assertion failed"}}).encode()
            self.send_response(400)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            print(f"fixture assertion: {exc!r}", file=sys.stderr, flush=True)


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(server.server_port, flush=True)
server.serve_forever()
