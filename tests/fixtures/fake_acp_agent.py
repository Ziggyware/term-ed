#!/usr/bin/env python3
"""A scripted ACP agent for tests and screenshots: streams a reply, edits a
file on disk (reporting a diff), shows a plan and asks for permission."""
import json, os, sys, time

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

def update(sid, u):
    send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": sid, "update": u}})

def recv():
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    return json.loads(line)

cwd = os.getcwd()
model = "mock/first-free"
stalled_prompt = None
while True:
    m = recv()
    meth, mid, p = m.get("method"), m.get("id"), m.get("params", {})
    if meth == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {
            "protocolVersion": 1,
            "agentCapabilities": {"promptCapabilities": {"embeddedContext": True}},
            "authMethods": [], "agentInfo": {"name": "fake", "title": "Test Agent"}}})
    elif meth == "session/new":
        cwd = p["cwd"]
        send({"jsonrpc": "2.0", "id": mid, "result": {"sessionId": "s1", "configOptions": [
            {"id": "model", "name": "Model", "category": "model", "type": "select",
             "currentValue": "mock/first-free",
             "options": [{"value": "mock/first-free", "name": "First free"},
                         {"value": "paid/pro", "name": "Pro Model"},
                         {"value": "mock/second-free", "name": "Step 5 (free)"}]}]}})
    elif meth == "session/set_config_option":
        model = p.get("value", model)
        send({"jsonrpc": "2.0", "id": mid, "result": {}})
    elif meth == "session/cancel":
        if stalled_prompt is not None:
            send({"jsonrpc": "2.0", "id": stalled_prompt, "result": {"stopReason": "cancelled"}})
            stalled_prompt = None
    elif meth == "session/prompt" and os.environ.get("FAKE_STALL") == model:
        # Simulate a rate-limited free model: say nothing until cancelled.
        stalled_prompt = mid
    elif meth == "session/prompt":
        sid = p["sessionId"]
        text = p["prompt"][0]["text"]
        update(sid, {"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "Reading calc.py to find the bug."}})
        update(sid, {"sessionUpdate": "plan", "entries": [
            {"content": "Read calc.py", "priority": "high", "status": "completed"},
            {"content": "Fix add()", "priority": "high", "status": "in_progress"}]})
        path = os.path.join(cwd, "calc.py")
        update(sid, {"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "read", "kind": "read", "status": "completed",
                     "locations": [{"path": path}]})
        # Ask before editing.
        send({"jsonrpc": "2.0", "id": 900, "method": "session/request_permission", "params": {
            "sessionId": sid, "toolCall": {"toolCallId": "t2", "title": "edit calc.py"},
            "options": [{"optionId": "once", "name": "Allow once", "kind": "allow_once"},
                        {"optionId": "always", "name": "Always", "kind": "allow_always"},
                        {"optionId": "no", "name": "Reject", "kind": "reject_once"}]}})
        answer = recv()
        allowed = answer.get("result", {}).get("outcome", {}).get("optionId") in ("once", "always")
        if allowed:
            old = open(path).read()
            new = old.replace("a - b", "a + b")
            open(path, "w").write(new)
            update(sid, {"sessionUpdate": "tool_call", "toolCallId": "t2", "title": "edit", "kind": "edit", "status": "completed",
                         "locations": [{"path": path}],
                         "content": [{"type": "diff", "path": path, "oldText": "    return a - b", "newText": "    return a + b"}]})
        for chunk in ["Fixed it: `add()` used ", "**subtraction**. ", "Now it returns `a + b`.\n\n```python\ndef add(a, b):\n    return a + b\n```"]:
            update(sid, {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": chunk}})
            time.sleep(0.02)
        update(sid, {"sessionUpdate": "plan", "entries": [
            {"content": "Read calc.py", "priority": "high", "status": "completed"},
            {"content": "Fix add()", "priority": "high", "status": "completed"}]})
        update(sid, {"sessionUpdate": "usage_update", "used": 12213, "size": 200000, "cost": {"amount": 0, "currency": "USD"}})
        send({"jsonrpc": "2.0", "id": mid, "result": {"stopReason": "end_turn"}})
    elif mid is not None and meth:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "nope"}})
