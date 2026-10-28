# Plugins

termed plugins add commands to the editor. A plugin is a folder with a
manifest and a program; termed starts the program, talks JSON-RPC 2.0 with it
one message per line, and routes its palette entries and keybindings to it.
That is the same transport as the Language Server Protocol and the Agent
Client Protocol, so a plugin can be written in anything that can read a line
of JSON: Python, Node, Go, Rust, a shell script.

## Where plugins live

Searched in this order (the first plugin with a given name wins):

| Folder | Use |
| --- | --- |
| `<project>/.termed/plugins/<name>/` | project-local plugins, committed with the repo |
| `~/.config/termed/plugins/<name>/` | plugins for all projects |
| `~/.local/share/termed/plugins/<name>/` | older XDG data location |
| `~/.cache/termed/plugins/<name>/` | bundled plugins, written on first run |

`Ctrl+Alt+P` (or **Plugins: Manage Plugins** in the palette) lists everything
found, with each plugin's state, and lets you reload, reinstall the bundled
set, and turn plugins on and off. The choice is stored in
`~/.config/termed/plugins.json`:

```json
{ "disabled": ["stats"] }
```

## Manifest

`plugin.json`:

```json
{
  "name": "text-tools",
  "version": "1.0.0",
  "description": "Sort and trim the buffer",
  "command": ["python3", "main.py"],
  "events": ["startup", "save"],
  "commands": [
    { "id": "sort", "title": "Sort Lines (A-Z)", "keys": "ctrl+alt+s" }
  ]
}
```

- `name` — unique, shown in the palette and used in `plugins.json`.
- `command` — argv. Relative paths run from the plugin folder. A single
  string is split on whitespace. The executable must be on `PATH`; a missing
  one is reported in the manager rather than failing silently.
- `events` — the notifications the plugin wants: `startup`, `open`, `save`.
- `commands[].keys` — optional binding such as `ctrl+alt+l`, `f5`,
  `shift+ctrl+pageup`. The editor's own bindings win when they collide (so
  `ctrl+s` always saves); the command is still in the palette, and
  `Ctrl+Alt+P` lists the plugin.

### VS Code extensions

A folder with `package.json` instead of `plugin.json` is read as a VS Code
extension: `contributes.commands` become palette entries, `publisher` is the
author, and `activationEvents` map to `startup` / `open`. The extension's
code is JavaScript for VS Code's own host, so it is listed as *"VS Code
extension: add a `termed` command to run it"* and its commands are not
offered until the package adds a bridge:

```json
{
  "name": "prettier-vscode",
  "displayName": "Prettier",
  "termed": { "command": ["node", "adapter.js"] },
  "contributes": { "commands": [{ "command": "prettier.format", "title": "Format" }] }
}
```

`adapter.js` is a small script that receives `command` requests and forwards
them to the extension (or reimplements the few commands you care about in
terms of `editor.text` / `editor.replace`). Anything that already speaks LSP
or ACP can be adapted the same way.

## Protocol

termed writes and reads JSON-RPC 2.0, one object per line.

**From termed to the plugin**

```json
{"jsonrpc":"2.0","method":"event","params":{"name":"save","params":{"path":"/p/a.rs"}}}
{"jsonrpc":"2.0","id":1,"method":"command","params":{"id":"sort","root":"/p","file":"/p/a.rs"}}
```

**From the plugin to termed** — a request with an `id`, answered by termed:

| Method | Params | Result |
| --- | --- | --- |
| `editor.text` | — | the active buffer |
| `editor.path` | — | its path or `null` |
| `editor.selection` | — | the selected text |
| `editor.insert` | `{"text"}` | `null` — insert at the caret |
| `editor.replace` | `{"text"}` | `null` — replace the buffer (undoable) |
| `editor.open` | `{"path", "line"?}` | `null` |
| `project.root` | — | the project folder |
| `project.files` | `{"limit"?}` | relative paths, `.gitignore` respected |
| `ui.toast` | `{"message", "kind"?}` | `null` — `info` (default), `success`, `error` |

Unknown methods return `null`, so a plugin written for a newer version
degrades instead of hanging. A request without an `id` is a notification and
gets no reply.

## Bundled plugins

These ship inside the binary and are installed to `~/.cache/termed/plugins`
on first run. They only need Python 3.

| Plugin | Commands |
| --- | --- |
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

in `<project>/.termed/plugins/counter/`. Press `Ctrl+Alt+P` → **Reload**, then
run **counter: Count Characters** from the palette.

Notes:

- Always answer a `command` request when it carries an `id`; termed uses the
  reply to know the command finished.
- Keep stdout for protocol messages only — write logs to stderr. termed
  ignores stderr unless the plugin exits with an error.
- The plugin process is started on first use and kept for the session.
  Disabling a plugin or quitting termed stops it.

## AI access

The Plugins title-bar menu / **Ctrl+Alt+P** now opens each plugin's command and
AI-access menu. Enabled, runnable plugins can be discovered with `list_plugins`
and executed with `run_plugin` by the built-in agent or through the editor's
authenticated MCP relay for external ACP agents. Every AI invocation requires
Allow Once; disabled, denied, missing or incompatible commands are rejected.

`run_plugin` accepts optional object `arguments` (passed as `params.arguments`)
and an optional workspace `file`. The host pins the target buffer while handling
editor callbacks, waits for the command's JSON-RPC result, and returns messages
and the resulting unsaved buffer. AI callers can use `read_editor` for fresh
in-memory state. Plugin errors, error toasts, exits and timeouts propagate as
tool failures. Plugin edits remain ordinary undoable editor changes.

Plugins execute with the user's local account privileges; this protocol and
approval flow are not a sandbox. See [agent configuration](agent-configuration.md)
for allowlists, inherited worker restrictions, lifecycle and transport details.
