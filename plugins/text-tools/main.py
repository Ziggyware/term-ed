#!/usr/bin/env python3
"""text-tools: line and text operations on the active buffer.

A termed plugin is any program that reads JSON-RPC 2.0 messages (one per
line) from stdin and writes replies to stdout. This one only needs the
standard library, so it works anywhere Python 3 does.
"""

import json
import sys

MAX_IO = 8 * 1024 * 1024


class Host:
    """Tiny synchronous client for the termed editor API."""

    def __init__(self):
        self.next_id = 1

    def send(self, message):
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()

    def call(self, method, params=None):
        rid = self.next_id
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": rid, "method": method,
                   "params": params or {}})
        while True:
            line = sys.stdin.readline()
            if not line:
                raise SystemExit(0)
            try:
                msg = json.loads(line)
            except ValueError:
                continue
            if msg.get("id") == rid and ("result" in msg or "error" in msg):
                return msg.get("result")

    def toast(self, message, kind="info"):
        self.send({"jsonrpc": "2.0", "method": "ui.toast",
                   "params": {"message": message, "kind": kind}})


host = Host()


def lines_of(text):
    return text.split("\n")


def join_lines(lines):
    return "\n".join(lines)


def command_sort(params):
    desc = params["id"] == "sort-desc"
    text = host.call("editor.text") or ""
    if not text:
        return
    lines = lines_of(text)
    # Sort the visible lines but keep a trailing newline in place.
    trailing = lines and lines[-1] == ""
    body = lines[:-1] if trailing else lines
    body.sort(key=lambda s: s.lower(), reverse=desc)
    out = join_lines(body) + ("\n" if trailing else "")
    if out == text:
        host.toast("Lines are already sorted")
        return
    host.call("editor.replace", {"text": out})
    host.toast("Sorted %d lines" % len(body), "success")


def command_dedupe():
    text = host.call("editor.text") or ""
    if not text:
        return
    lines = lines_of(text)
    seen = set()
    out = []
    for line in lines:
        if line in seen and line.strip():
            continue
        seen.add(line)
        out.append(line)
    removed = len(lines) - len(out)
    if not removed:
        host.toast("No duplicate lines")
        return
    host.call("editor.replace", {"text": join_lines(out)})
    host.toast("Removed %d duplicate line(s)" % removed, "success")


def command_trim():
    text = host.call("editor.text") or ""
    if not text:
        return
    lines = lines_of(text)
    trimmed = [line.rstrip() if not line.endswith("\r") else line.rstrip() for line in lines]
    changed = sum(1 for a, b in zip(lines, trimmed) if a != b)
    if not changed:
        host.toast("No trailing whitespace")
        return
    host.call("editor.replace", {"text": join_lines(trimmed)})
    host.toast("Trimmed %d line(s)" % changed, "success")


def command_case(upper):
    text = host.call("editor.text") or ""
    if not text:
        return
    out = text.upper() if upper else text.lower()
    host.call("editor.replace", {"text": out})
    host.toast("Converted to %s case" % ("UPPER" if upper else "lower"), "success")


def command_json(minify):
    text = host.call("editor.text") or ""
    if not text.strip():
        host.toast("Nothing to format")
        return
    try:
        value = json.loads(text)
    except ValueError as err:
        host.toast("Not valid JSON: %s" % err, "error")
        return
    if minify:
        out = json.dumps(value, separators=(",", ":"))
    else:
        out = json.dumps(value, indent=2, ensure_ascii=False)
    host.call("editor.replace", {"text": out})
    host.toast("JSON %s" % ("minified" if minify else "formatted"), "success")


COMMANDS = {
    "sort": command_sort,
    "sort-desc": command_sort,
    "dedupe": lambda params: command_dedupe(),
    "trim": lambda params: command_trim(),
    "upper": lambda params: command_case(True),
    "lower": lambda params: command_case(False),
    "json-pretty": lambda params: command_json(False),
    "json-minify": lambda params: command_json(True),
}


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        method = msg.get("method")
        params = msg.get("params") or {}
        rid = msg.get("id")
        if method == "command":
            handler = COMMANDS.get(params.get("id"))
            if handler is None:
                if rid is not None:
                    host.send({"jsonrpc": "2.0", "id": rid,
                               "error": {"code": -32601, "message": "unknown command"}})
                continue
            try:
                handler(params)
            except Exception as err:  # keep the plugin alive
                host.toast("text-tools: %s" % err, "error")
            if rid is not None:
                host.send({"jsonrpc": "2.0", "id": rid, "result": {"ok": True}})
        elif rid is not None:
            host.send({"jsonrpc": "2.0", "id": rid, "result": None})


if __name__ == "__main__":
    main()
