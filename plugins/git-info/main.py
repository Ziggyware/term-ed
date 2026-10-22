#!/usr/bin/env python3
"""git-info: the current branch and how dirty the working tree is."""

import json
import subprocess
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


def git(root, *args):
    try:
        out = subprocess.run(["git", "-C", root, *args], capture_output=True,
                             text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return None
    if out.returncode != 0:
        return None
    return out.stdout


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
        root = host.call("project.root") or "."
        branch = git(root, "rev-parse", "--abbrev-ref", "HEAD")
        if not branch:
            host.toast("Not a git repository", "error")
        else:
            status = git(root, "status", "--porcelain") or ""
            changed = len([l for l in status.splitlines() if l.strip()])
            ahead = git(root, "rev-list", "--count", "@{u}..HEAD") or "0"
            ahead = ahead.strip()
            msg = "%s · %d changed file(s)" % (branch.strip(), changed)
            if ahead not in ("", "0"):
                msg += " · %s commit(s) ahead" % ahead
            host.toast(msg, "success")
        if rid is not None:
            host.send({"jsonrpc": "2.0", "id": rid, "result": {"ok": True}})


if __name__ == "__main__":
    main()
