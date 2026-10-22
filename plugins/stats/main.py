#!/usr/bin/env python3
"""stats: word, character and line counts for the buffer or the selection."""

import json
import sys


class Host:
    def __init__(self):
        self.next_id = 1

    def send(self, message):
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()

    def call(self, method, params=None):
        rid = self.next_id
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params or {}})
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
        self.send({"jsonrpc": "2.0", "method": "ui.toast", "params": {"message": message, "kind": kind}})


host = Host()


def describe(text, label):
    chars = len(text)
    words = len(text.split())
    lines = text.count("\n") + (1 if text and not text.endswith("\n") else 0)
    longest = max((len(line) for line in text.split("\n")), default=0)
    host.toast("%s: %d words, %d chars, %d lines, longest %d" % (label, words, chars, lines, longest), "success")


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        rid = msg.get("id")
        if msg.get("method") != "command":
            if rid is not None:
                host.send({"jsonrpc": "2.0", "id": rid, "result": None})
            continue
        want = (msg.get("params") or {}).get("id")
        if want == "selection":
            text = host.call("editor.selection") or ""
            if not text:
                host.toast("Nothing selected")
            else:
                describe(text, "Selection")
        else:
            text = host.call("editor.text") or ""
            describe(text, "Document")
        if rid is not None:
            host.send({"jsonrpc": "2.0", "id": rid, "result": {"ok": True}})


if __name__ == "__main__":
    main()
