#!/usr/bin/env python3
"""Stand-ins for a keyed OpenAI-compatible server (/v1/...) and Anthropic's
Messages API (/anthropic/v1/...). Both demand an API key. The Anthropic
mock streams a signed thinking block, a valid edit_file call and a call
with broken JSON, then checks that the next request echoes the thinking
block unchanged and answers both calls in one user message."""
import json
from http.server import BaseHTTPRequestHandler, HTTPServer

KEY = "sk-test-123"

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"

    def log_message(self, *a):
        pass

    def json(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def authed(self):
        if self.path.startswith("/anthropic"):
            ok = self.headers.get("x-api-key") == KEY and self.headers.get("anthropic-version") == "2023-06-01"
        else:
            ok = self.headers.get("Authorization") == "Bearer " + KEY
        if not ok:
            self.json(401, {"error": {"type": "authentication_error", "message": "invalid x-api-key"}})
        return ok

    def do_GET(self):
        if not self.authed():
            return
        if self.path.startswith("/anthropic/v1/models"):
            caps = {"thinking": {"supported": True, "types": {"adaptive": {"supported": True}}},
                    "effort": {"supported": True, "high": {"supported": True}}}
            self.json(200, {"data": [
                {"id": "claude-opus-5-5", "display_name": "Claude Opus 5.5", "max_tokens": 128000, "capabilities": caps},
            ]})
        else:
            self.json(200, {"data": [{"id": "text-embedding-3"}, {"id": "local-coder"}]})

    def sse(self, events):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        for ev in events:
            self.wfile.write(("event: %s\ndata: %s\n\n" % (ev["type"], json.dumps(ev))).encode())

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if not self.authed():
            return
        if self.path == "/anthropic/v1/messages":
            return self.anthropic(req)
        # OpenAI-compatible: edit, then answer.
        if not any(m.get("role") == "tool" for m in req["messages"]):
            args = json.dumps({"path": "calc.py", "old_text": "a - b", "new_text": "a + b"})
            chunks = [{"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "t1", "function": {"name": "edit_file", "arguments": args}}]}}]}]
        else:
            chunks = [{"choices": [{"delta": {"content": "Fixed via OpenAI-compatible."}}]},
                      {"choices": [], "usage": {"total_tokens": 50}}]
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        for c in chunks:
            self.wfile.write(("data: " + json.dumps(c) + "\n\n").encode())
        self.wfile.write(b"data: [DONE]\n\n")

    def anthropic(self, req):
        problems = []
        if req.get("thinking") != {"type": "adaptive", "display": "summarized"}:
            problems.append("thinking")
        if req.get("output_config") != {"effort": "high"}:
            problems.append("effort")
        if req.get("fallbacks") != "default" or self.headers.get("anthropic-beta") != "server-side-fallback-2026-07-01":
            problems.append("fallbacks")
        if not req.get("stream") or not all(t.get("eager_input_streaming") for t in req["tools"]):
            problems.append("streaming")
        msgs = req["messages"]
        start = {"type": "message_start", "message": {"id": "m1", "role": "assistant", "content": [],
                 "usage": {"input_tokens": 100, "output_tokens": 0}}}
        if len(msgs) == 1:
            args = json.dumps({"path": "calc.py", "old_text": "a - b", "new_text": "a + b"})
            return self.sse([
                start,
                {"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}},
                {"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Wrong operator."}},
                {"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "SIG-abc"}},
                {"type": "content_block_stop", "index": 0},
                {"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "tu1", "name": "edit_file", "input": {}}},
                {"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": args[:15]}},
                {"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": args[15:]}},
                {"type": "content_block_stop", "index": 1},
                {"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "tu2", "name": "write_file", "input": {}}},
                {"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": '{"path": "x.py", "content": "oops'}},
                {"type": "content_block_stop", "index": 2},
                {"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 40}},
                {"type": "message_stop"},
            ])
        a, u = msgs[1], msgs[2]
        if a["role"] != "assistant" or a["content"][0] != {"type": "thinking", "thinking": "Wrong operator.", "signature": "SIG-abc"}:
            problems.append("thinking echo")
        if [b["type"] for b in a["content"]] != ["thinking", "tool_use", "tool_use"]:
            problems.append("assistant blocks")
        res = {r["tool_use_id"]: r for r in u["content"]} if u["role"] == "user" else {}
        if set(res) != {"tu1", "tu2"}:
            problems.append("tool results")
        elif not res["tu2"].get("is_error") or "INVALID_JSON" not in res["tu2"]["content"] or res["tu1"].get("is_error"):
            problems.append("invalid json result")
        text = "CHECK FAILED: " + ", ".join(problems) if problems else "Fixed via Claude."
        self.sse([
            start,
            {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}},
            {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}},
            {"type": "content_block_stop", "index": 0},
            {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 5}},
            {"type": "message_stop"},
        ])

srv = HTTPServer(("127.0.0.1", 0), H)
print(srv.server_address[1], flush=True)
srv.serve_forever()
