#!/usr/bin/env python3
"""Router-shaped SSE: complete unindexed calls, nested tasks, runtime functions.
Assertions are provider-side too, so malformed instructions/schema/history fail
rather than succeeding via a permissive canned completion. No external network.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        try:
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            assert self.path == "/v1/chat/completions", self.path
            assert request["stream"] is True
            assert 256 <= request["max_tokens"] <= 4096
            system = request["messages"][0]["content"]
            assert "ROOT-RULE-MARKER" in system
            assert "INHERITED-SKILL-MARKER" in system
            tools = {t["function"]["name"] for t in request["tools"]}
            results = [m for m in request["messages"] if m["role"] == "tool"]
            n = len(results)
            user = next(m["content"] for m in request["messages"] if m["role"] == "user")
            calls, answer = [], None
            if "LEAF-RULE-MARKER" in system:
                assert "COORDINATOR-RULE-MARKER" in system
                assert not ({"write_file", "edit_file", "run_command", "run_plugin", "delegate_task"} & tools)
                assert request["max_tokens"] == 1024
                brief = json.loads(user.split("TASK BRIEF (JSON):\n", 1)[1])
                assert brief["root_goal"].startswith("ROOT-GOAL-MARKER")
                assert brief["depth"] == 2 and brief["parent_id"]
                assert brief["lineage"] == ["coordinator", "reviewer"]
                assert "Do not modify notes" in brief["constraints"]
                assert brief["ancestor_objectives"] == ["Coordinate evidence review."]
                assert "evidence-marker" in json.dumps(brief["context"])
                assert all(not r["tool_call_id"].startswith(("root-", "coord-")) for r in results)
                if n == 0:
                    calls = [("read_editor", {"file": "notes.txt"})]
                elif n == 1:
                    assert "unsaved-live-marker" in results[0]["content"]
                    calls = [("report_progress", {"message": "Read the live unsaved evidence; no files changed."})]
                else:
                    answer = "Leaf verified unsaved-live-marker in notes.txt; no edits or commands were run."
            elif "COORDINATOR-RULE-MARKER" in system:
                assert "delegate_task" in tools and "run_command" not in tools
                assert request["max_tokens"] == 1024
                brief = json.loads(user.split("TASK BRIEF (JSON):\n", 1)[1])
                assert brief["depth"] == 1 and brief["parent_id"] is None
                assert brief["root_goal"].startswith("ROOT-GOAL-MARKER")
                assert "evidence-marker" in json.dumps(brief["context"])
                assert all(not r["tool_call_id"].startswith("root-") for r in results)
                if n == 0:
                    calls = [("list_functions", {})]
                elif n == 1:
                    assert "fn_normalize" in results[0]["content"]
                    calls = [("delegate_task", {"agent": "reviewer", "task": "Verify the live notes evidence.", "acceptance_criteria": "Cite the marker and state whether checks actually ran.", "constraints": "No commands.", "context": "Use read_editor: the notes buffer may have unsaved updates.", "context_files": ["notes.txt"]})]
                else:
                    assert '"depth":2' in results[-1]["content"]
                    assert "Leaf verified" in results[-1]["content"]
                    answer = "Coordinator collected leaf evidence: unsaved-live-marker; no edits or commands."
            else:
                assert "ROOT-GOAL-MARKER" in user
                if n == 0:
                    calls = [
                        ("report_progress", {"message": "Preparing a bounded evidence review."}),
                        ("define_function", {"name": "fn_normalize", "description": "Normalize a small text value", "input_schema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}, "input": "/text", "steps": [{"op": "trim"}, {"op": "lowercase"}]})
                    ]
                elif n == 2:
                    assert "fn_normalize" in tools
                    assert '"dynamic_function"' in results[1]["content"]
                    calls = [("fn_normalize", {"text": "  Evidence-Marker  "})]
                elif n == 3:
                    assert results[2]["content"] == "evidence-marker"
                    calls = [("delegate_task", {"agent": "coordinator", "task": "Coordinate evidence review.", "acceptance_criteria": "Collect an evidence-based specialist result and unresolved limitations.", "constraints": "Do not modify notes.", "context": "Only this investigation is in scope.", "context_files": ["notes.txt"]})]
                elif n == 4:
                    assert "Coordinator collected" in results[3]["content"]
                    calls = [("task_status", {})]
                else:
                    status = json.loads(results[-1]["content"])
                    assert len(status["tasks"]) == 2
                    assert all(t["status"] == "completed" for t in status["tasks"])
                    assert all(t["result"] for t in status["tasks"])
                    answer = "Nested audit finished: generated function called, coordinator and grandchild joined, live evidence verified. No edits or commands."
            prefix = "leaf-" if "LEAF-RULE-MARKER" in system else "coord-" if "COORDINATOR-RULE-MARKER" in system else "root-"
            for name, _ in calls:
                assert name in tools, (name, tools)
            delta = {"tool_calls": [{"id": f"{prefix}{n+i}", "type": "function", "function": {"name": name, "arguments": json.dumps(args)}} for i, (name, args) in enumerate(calls)]} if calls else {"content": answer}
            # Exact router layout: full unindexed tool_calls in one event, then
            # a separate finish event and [DONE], including reasoning_content.
            chunks = [
                {"choices": [{"delta": {"reasoning_content": "Checking task scope."}, "finish_reason": None}]},
                {"choices": [{"delta": delta, "finish_reason": None}]},
                {"choices": [{"delta": {}, "finish_reason": "tool_calls" if calls else "stop"}]},
            ]
            data = ("".join("data: " + json.dumps(c) + "\n\n" for c in chunks) + "data: [DONE]\n\n").encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        except Exception as exc:
            message = json.dumps({"error": {"message": f"mock assertion: {exc}"}}).encode()
            self.send_response(400)
            self.send_header("Content-Length", str(len(message)))
            self.end_headers()
            self.wfile.write(message)
            print(f"mock assertion: {exc!r}", file=sys.stderr, flush=True)


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(server.server_port, flush=True)
server.serve_forever()
