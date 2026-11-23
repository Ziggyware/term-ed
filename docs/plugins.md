# Plugins

termed plugins add commands to the editor. A plugin is a folder with a manifest and a program; termed starts the program, talks JSON-RPC 2.0 with it one message per line, and routes its palette entries and keybindings to it.

## Where plugins live

Searched in this order (the first plugin with a given name wins):

| Folder | Use |
|---|---|
| `<project>/.termed/plugins/<name>/` | project-local plugins, committed with the repo |
| `~/.config/termed/plugins/<name>/` | plugins for all projects |
| `~/.local/share/termed/plugins/<name>/` | older XDG data location |
| `~/.cache/termed/plugins/<name>/` | bundled plugins, written on first run |

`Ctrl+Alt+P` (or **Plugins: Manage Plugins** in the palette) lists everything found, with each plugin's state, and lets you reload, reinstall the bundled set, and turn plugins on and off. The choice is stored in `~/.config/termed/plugins.json`:

```json
{ "disabled": ["stats"] }
```

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

- `name` — unique, shown in the palette and used in `plugins.json`.
- `command` — argv. Relative paths run from the plugin folder.
- `commands` — palette entries the plugin exposes.

## Bundled plugins

These ship inside the binary and are installed to `~/.cache/termed/plugins` on first run. They only need Python 3.

| Plugin | Commands |
|---|---|
| `markdown-preview` | render the live Markdown buffer to a styled page and open it in the browser (`Ctrl+Shift+M`), or show it read-only in the editor. |
| `text-tools` | sort (A-Z / Z-A), remove duplicate lines, trim trailing whitespace, UPPER/lower case, format/minify JSON — `Ctrl+Alt+L` sorts |
| `todo` | list and count `TODO` / `FIXME` / `HACK` / `XXX` markers in the project, then jump to the first one — `Ctrl+Alt+T` |
| `stats` | words, characters, lines and longest line for the document or the selection |
| `git-info` | current branch, changed files and commits ahead — `Ctrl+Alt+G` |

## Writing one

A minimal plugin, in Python, that reports the buffer size:

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
    send({"jsonrpc": "2.0", "id": 1, "method": "editor.text", "params": {}})
    text = ""
    while True:
        reply = json.loads(sys.stdin.readline())
        if reply.get("id") == 1:
            text = reply.get("result") or ""
            break
    send({"jsonrpc": "2.0", "id": 2, "method": "ui.toast",
          "params": {"message": "%d chars" % len(text)}})
    send({"jsonrpc": "2.0", "id": msg["id"], "result": {"ok": True}})
```

with

```json
{
  "name": "counter",
  "command": ["python3", "main.py"],
  "commands": [{ "id": "count", "title": "Count Characters" }]
}
```

in `<project>/.termed/plugins/counter/`. Press `Ctrl+Alt+P` → **Reload**, then run **counter: Count Characters** from the palette.
