#!/usr/bin/env python3
"""Deterministic OpenAI-compatible provider for plugin/skill/delegation tests.
No external network, API keys, models, or environment mutation needed."""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        data = json.dumps({"data": [{"id": "test/automation", "name": "Test", "supported_parameters": ["tools"]}]}).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        tools = {t["function"]["name"] for t in request["tools"]}
        system = request["messages"][0]["content"]
        results = [m for m in request["messages"] if m["role"] == "tool"]
        n = len(results)
        if "SPECIALIST-REVIEW" in system:
            assert not ({"write_file", "edit_file", "run_command", "run_plugin", "delegate_task"} & tools), tools
            assert "SKILL-MARKER" in system
            assert not any("tool_call_id" in m and m["tool_call_id"].startswith("parent") for m in request["messages"])
            if n == 0:
                call = ("read_file", {"path": "notes.txt"})
            else:
                call = None
                answer = "Specialist verified notes with an isolated read-only session."
        else:
            assert "PARENT-AUTOMATION" in system and "SKILL-MARKER" in system
            sequence = [
                ("list_plugins", {}),
                ("run_plugin", {"plugin": "text-tools", "command": "sort", "file": "notes.txt"}),
                ("list_skills", {}),
                ("read_skill", {"id": "policy"}),
                ("list_sub_agents", {}),
                ("delegate_task", {"agent": "reviewer", "task": "Inspect notes.txt and report your findings."}),
            ]
            call = sequence[n] if n < len(sequence) else None
            if call is None:
                assert "Specialist verified" in results[-1]["content"], results[-1]
                assert "SKILL-MARKER" in results[3]["content"]
                assert '"unsaved":true' in results[1]["content"]
                answer = "Finished: plugin sorted the live buffer, skill loaded, specialist verified."
        if call:
            name, arguments = call
            assert name in tools
            chunk = {"choices": [{"delta": {"tool_calls": [{"index": 0, "id": ("child" if "SPECIALIST-REVIEW" in system else "parent") + str(n), "function": {"name": name, "arguments": json.dumps(arguments)}}]}}]}
        else:
            chunk = {"choices": [{"delta": {"content": answer}}]}
        body = ("data: " + json.dumps(chunk) + "\n\ndata: [DONE]\n\n").encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


if __name__ == "__main__":
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(server.server_address[1], flush=True)
    server.serve_forever()
