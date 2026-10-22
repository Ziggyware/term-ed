#!/usr/bin/env python3
"""todo: scan the project for TODO / FIXME / HACK / XXX comments.

Uses `project.files` and `project.root` from the termed API, so it respects
the same file set the editor knows about, then opens the first hit with
`editor.open`.
"""

import json
import os
import re
import sys

MARKERS = re.compile(r"\b(TODO|FIXME|HACK|XXX)\b")
LIMIT = 4000
MAX_BYTES = 512 * 1024
SKIP_DIRS = {".git", "node_modules", "target", "bin", "obj", "dist", "build", ".venv"}
BINARY = (".png", ".jpg", ".jpeg", ".gif", ".pdf", ".zip", ".gz", ".exe", ".dll", ".so", ".dylib", ".woff", ".woff2", ".ico")


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


def scan(root, relative_paths):
    hits = []
    for rel in relative_paths[:LIMIT]:
        if rel.lower().endswith(BINARY):
            continue
        parts = rel.split("/")
        if any(p in SKIP_DIRS for p in parts[:-1]):
            continue
        path = os.path.join(root, *parts)
        try:
            if os.path.getsize(path) > MAX_BYTES:
                continue
            with open(path, "r", encoding="utf-8", errors="replace") as fh:
                for number, line in enumerate(fh, 1):
                    m = MARKERS.search(line)
                    if m:
                        hits.append((rel, number, m.group(1), line.strip()[:160]))
        except OSError:
            continue
    return hits


def collect():
    root = host.call("project.root") or "."
    files = host.call("project.files", {"limit": LIMIT}) or []
    return root, scan(root, files)


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
        if method != "command":
            if rid is not None:
                host.send({"jsonrpc": "2.0", "id": rid, "result": None})
            continue
        root, hits = collect()
        if not hits:
            host.toast("No TODO / FIXME markers in %d files" % min(len(host.call("project.files", {"limit": LIMIT}) or []), LIMIT))
        elif params.get("id") == "count":
            counts = {}
            for _, _, marker, _ in hits:
                counts[marker] = counts.get(marker, 0) + 1
            summary = ", ".join("%s %d" % (k, v) for k, v in sorted(counts.items()))
            host.toast("%d markers: %s" % (len(hits), summary), "success")
        else:
            rel, number, marker, text = hits[0]
            host.toast("%d markers, first: %s:%d  %s" % (len(hits), rel, number, marker), "success")
            host.call("editor.open", {"path": os.path.join(root, *rel.split("/")), "line": number})
        if rid is not None:
            host.send({"jsonrpc": "2.0", "id": rid, "result": {"hits": len(hits)}})


if __name__ == "__main__":
    main()
