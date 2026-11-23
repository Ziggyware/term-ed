# Agent Configuration Reference

## Primary Agent Configuration

| Setting | Meaning |
|---|---|
| Backend | `builtin` (default), `custom`, or exact ACP agent name; legacy `auto` selects built-in |
| Custom ACP command | argv string with quoted args; no shell expansion |
| Model | Provider model ID; empty = provider default or first discovered |
| API base URL | OpenAI-compatible base URL including `/v1`; empty = selected provider |
| Local ONNX endpoint | Local server base URL for `local` provider; default `http://127.0.0.1:8080/v1` |
| API key environment variable | Name of env var holding the key |
| Instructions | Role, conventions, additional instructions (use JSON for multi-line) |
| Step budget | 1–100 model/tool rounds per turn (default 30) |
| Read-only | Agent cannot edit/write files, run commands, or execute plugins |
| Command permissions | `ask` (default), `deny`, `risk_assessed`, `allow` — destructive commands always blocked |
| AI plugin tools | Expose plugin discovery/execution to the agent |
| Plugin allowlist | Comma-separated enabled plugin names; empty = all enabled except explicit denials |
| Attached skills | Comma-separated enabled skill IDs to include on each prompt |
| Sub-agent delegation | Whether the primary AI can delegate to specialists |
| Delegations per turn | 1–32 total tasks for whole root tree (default 4); descendants share budget |
| Task timeout | 10–1800 seconds (default 300) for root turn and inherited deadlines |
| Attach editor context | Include active file and selection in prompts and worker briefs |
| Built-in provider | `router` (default), `kilo`, `custom`, `local` (on-device ONNX); `onnx` normalizes to `local` |
| Maximum agent depth | 1–3 (default 2): root=0, child=1, grandchild=2; ancestors may lower the cap |
| Concurrent tasks | 1–8 active tasks (default 4), including waiting coordinators |
| Overwatch stall timeout | 15–600 seconds (default 150); child/approval/plugin waits exempt from idle detection |
| Pure generated functions | Enable bounded task-scoped data transforms; no code execution |
| Extended AI toolkit | Expose 100 host tools in 10 groups; write tools hidden from read-only agents |
| Toolkit groups | Comma-separated: `workspace`, `text`, `buffers`, `edit`, `filesystem`, `search`, `git`, `ui`, `code`, `session`; empty = all |
| Model output token budget | 256–16384 (default 4096), inherited restrictively |

## Minimal Example

```json
{
  "version": 2,
  "editor": { "minimap": false, "word_wrap": true },
  "agent": {
    "backend": "builtin",
    "provider": "router",
    "model": "free:code",
    "instructions": "Prefer small changes. Explain how to verify each change.",
    "sub_agents": true,
    "max_delegations": 3,
    "toolkit": true,
    "toolkit_groups": ["workspace", "text", "buffers", "search", "git"]
  }
}
```
