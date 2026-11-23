# Plugins

termed plugins add commands via a manifest and program.

## Where plugins live

| Folder | Use |
|---|---|
| `<project>/.termed/plugins/<name>/` | project‑local plugins |
| `~/.config/termed/plugins/<name>/` | all‑project plugins |
| `~/.local/share/termed/plugins/<name>/` | legacy XDG location |
| `~/.cache/termed/plugins/<name>/` | bundled plugins |

## Manifest

`plugin.json`:

```json
{
  "name": "text-tools",
  "command": ["python3", "main.py"],
  "commands": [
    { "id": "sort", "title": "Sort Lines (A-Z)" }
  ]
}
```

## Bundled plugins

| Plugin | Commands |
|---|---|
| `markdown-preview` | render Markdown to HTML (Ctrl+Shift+M) |
| `text-tools` | sort, dedupe, trim, case‑change, JSON format (Ctrl+Alt+L) |
| `todo` | list/todo items, jump to first (Ctrl+Alt+T) |
| `stats` | words, chars, lines, longest (Ctrl+Alt+S) |
| `git-info` | branch, changes, ahead (Ctrl+Alt+G) |

## Writing one

Minimal Python plugin:

```python
#!/usr/bin/env python3
import json, sys

def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    msg = json.loads(line)
    if msg.get("method") != "command":
        continue
    send({"jsonrpc":"2.0","id":1,"method":"editor.text","params":{}})
    text = ""
    while True:
        reply = json.loads(sys.stdin.readline())
        if reply.get("id") == 1:
            text = reply.get("result") or ""
            break
    send({"jsonrpc":"2.0","id":2,"method":"ui.toast","params":{"message": "%d chars" % len(text)}})
    send({"jsonrpc":"2.0","id":msg["id"],"result":{"ok":True}})
```

with

```json
{
  "name": "counter",
  "command": ["python3","main.py"],
  "commands": [{ "id":"count","title":"Count Characters" }]
}
```

in `<project>/.termed/plugins/counter/`. Press `Ctrl+Alt+P` → **Reload**, then run **counter: Count Characters**.