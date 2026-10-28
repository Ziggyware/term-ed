# NoKey Router, supervised task trees and generated functions

## Connect once, without putting the key in chat

The built-in Rust agent defaults to **https://nokey-router.ziggyware.deno.net/v1**
and the `free:code` model. termed requires a `ROUTER_API_KEY`, even though the
router's upstream routing uses free models. No external agent install is needed.

**In the editor:**

1. Open **Settings → AI agent configuration** (or the agent panel's **⚙**).
2. Choose **Save Router API Key…**, enter/paste the raw key in the masked field,
   then press Enter. Esc cancels without saving. The editor saves the credential
   locally and selects the built-in NoKey Router in your user settings, retaining
   your other permissions/budgets and an already-selected router model. Do not
   put the secret in the separate **API key environment variable** field.
3. Send a task. Authentication and provider failures appear in the transcript;
   saving a key does not validate it or start a model request.

If selecting the router fails because user settings cannot be saved, the
editor reports that separately: the key is still saved locally. Fix the settings
and choose **Use NoKey Router**. That action can also switch an existing
configuration to the router without re-entering the key. It selects `builtin` + `router` +
`free:code` and preserves your other permissions/budgets.

**From a local terminal:**

```sh
termed --router-login    # hidden input; saves the key and exits
termed --router         # select the router for this launch
termed --router-logout   # remove the saved key and exit
```

`--router-login` also imports an already exported `ROUTER_API_KEY`, or reads stdin
when piped. It does not change settings; the router is the default, or you can
launch with `--router` or choose **Use NoKey Router** if you selected another provider. There is deliberately **no
key-valued command-line argument**. An existing nonempty `ROUTER_API_KEY` takes
precedence over the saved key at runtime; the editor warns if that override is
active when saving a key. Removing the saved file does not unset the override.

### Storage and origin restrictions

- The key lives at `$XDG_CONFIG_HOME/termed/credentials.json`, normally
  `~/.config/termed/credentials.json`, **not** `.termed/settings.json`.
- On Unix, writes use a private `0700` directory and `0600` temporary file,
  sync the file, then replace it by rename. Symlinks, hard-linked/shared-mode
  credential files, and credential locations inside Git checkouts are refused.
  Other platforms rely on their user-directory ACLs; this implementation does
  not install a Windows ACL or integrate with an OS keychain.
- This is **plaintext, not encrypted storage**. Use a protected user account,
  disk encryption or an environment override if that does not meet your needs.
  Do not put the config directory in a repository or unprotected cloud sync.
- Saved router credentials are released only for the exact pinned HTTPS `/v1`
  endpoint. HTTP, lookalike hosts, other paths and alternate origins cannot
  receive `ROUTER_API_KEY`. Router/authenticated requests do not follow redirects.
  Anonymous legacy Kilo discovery/requests retain bounded redirect support.
- Keys are not serialized into preferences, task briefs or generated schemas.
  Resolved values are redacted from provider errors. Native agent file, search,
  skill and editor-context APIs exclude the credential file and its aliases;
  the editor directs users to masked settings instead of opening that file.
- Managed shell, plugin and external-agent processes do not inherit
  `ROUTER_API_KEY`. They are nevertheless **trusted local executables**, not a
  sandbox: an approved program running as you can access your plaintext files.

The model menu offers known aliases without requiring an authenticated catalog
fetch: `free:code` (default), `free:auto`, `free:fast`, `free:smart`,
`free:reasoning`, and `openrouter/free`. An explicit model ID can also be set.
Actual availability, rate limits and function support depend on the deployed
router and its upstreams. Its buffered SSE format is supported, including
`reasoning_content` and multiple complete, unindexed tool calls. OpenAI-style
indexed fragments, object-form tool arguments, and complete JSON responses from
compatible providers that ignore streaming are also supported. Missing or reused
tool-call IDs are made unique across the conversation. Partial/oversized/malformed
responses do not execute incomplete tool arguments.

The legacy anonymous Kilo path and custom OpenAI-compatible endpoints remain
available. Custom providers use a separately named environment variable, not
the saved router key. External ACP backends manage their own provider setup;
choose the built-in backend to use this integration.

## Children and grandchildren

New configurations include three editable read-only profiles:

- **Reviewer**: leaf for code/safety review.
- **Test planner** (`tester`): leaf for test design and verification planning.
- **Coordinator**: may delegate focused investigations to those leaves and must
  collect their results before returning.

An existing saved `sub_agents` array is not replaced or silently merged. Add a
coordinator through **Manage Sub-agents** and enable its **Sub-agent delegation**
setting if your saved profiles predate this change. Both the root and a
coordinator must permit delegation. A leaf cannot gain that capability by
inventing a tool call.

### Default limits

| Setting | Default | Allowed values / enforcement |
| --- | --- | --- |
| `max_agent_depth` | 2 | 1–3; root=0, child=1, grandchild=2; every ancestor may lower the cap |
| `max_delegations` | 4 | 1–32 tasks for the whole root turn, including descendants; specialist branch caps may be smaller |
| `max_concurrent_tasks` | 4 | 1–8 live specialists, including waiting coordinators; excess requests fail immediately, not queue indefinitely |
| `task_timeout_secs` | 300 | 10–1800; root turn deadline, with each child deadline no later than its parent's |
| `stall_timeout_secs` | 150 | 15–600 without agent activity; child/approval/plugin waits are exempt from idle detection, not absolute deadlines |
| `max_steps` | 30 | 1–100 model rounds; default reviewer/tester=12, coordinator=18; inherited minimum |
| `max_output_tokens` | 4096 | 256–16384 completion tokens; inherited minimum |
| `generated_tools` | true | Pure interpreted transformations only; inherited capability intersection |

Builtin tool calls execute in order; this is not a parallel batch scheduler or a
background always-running agent service. The active-task cap covers nesting and
concurrent bridge callers. Reusing a profile already in the current ancestry is
rejected, preventing coordinator→coordinator cycles.

For example, this partial settings file selects the router and permits a bounded
root→coordinator→reviewer tree; it contains **no secret**:

```json
{
  "version": 2,
  "agent": {
    "backend": "builtin",
    "provider": "router",
    "model": "free:code",
    "instructions": "Prefer small changes. Cite evidence. Never claim checks not actually run.",
    "sub_agents": true,
    "max_agent_depth": 2,
    "max_delegations": 4,
    "max_concurrent_tasks": 4,
    "task_timeout_secs": 300,
    "stall_timeout_secs": 150,
    "max_output_tokens": 4096,
    "generated_tools": true
  }
}
```

### What a delegate receives

`delegate_task` accepts `agent`, `task`, optional `acceptance_criteria`,
`constraints`, concise `context`, and `context_files`. The host constructs a
brief containing the root goal, ancestor objectives and acceptance criteria,
current objective, lineage/IDs/depth, inherited constraints, capabilities,
remaining time, relevant reference snapshots and a fresh live-buffer excerpt.
It does **not** copy the parent's entire conversation/tool history.

Project and ancestor instructions are carried in the worker's system context;
the specialist role cannot widen programmatic restrictions. Required skill
snapshots propagate without rereading/mutating an ancestor's instructions.
Additional role skills must be enabled/readable. Missing skills or a combined
96 KiB skill budget overflow fail explicitly, rather than silently omitting them.

Task text is limited to 32 KiB; criteria/constraints to 8 KiB each; parent notes
to 16 KiB. Brief reference context is capped at 48 KiB, with up to six explicitly
selected text files and 12 KiB excerpts per file. Omit `context_files` to inherit
references; an explicit list curates them, and `[]` selects none. Ancestor notes,
goals and rules still propagate. Oversized context is rejected rather than
silently dropping ancestor findings. All snapshots are labeled as evidence,
not instructions; unsaved buffers require `read_editor`, not a stale disk read.

Before a request, the agent can omit older **complete** conversation exchanges
with an explicit context-window notice. The current goal/brief and system rules
are retained; assistant tool calls and their results stay together. The complete
request has a 480 KiB safety ceiling. These are byte budgets, not exact model
token counts or guarantees that an upstream's context window is large enough.

### Overwatch and cancellation

The agent panel shows an overwatch clock/counter. Worker cards include names,
IDs and depth; starts, progress, failures and completion are visible. **Show
Agent Overwatch** in AI settings displays the bounded task ledger, including
parent IDs, phases, deadlines, progress and terminal result excerpts.

Agents can call:

- `report_progress(message)`: record evidence or a blocker, without extending
  any deadline or granting permissions.
- `task_status()`: root sees the ledger; a worker sees only its subtree.
- `cancel_task(task_id)`: cancel a descendant and all of its descendants.
  Workers cannot cancel themselves, ancestors or siblings.

Failed, stalled, expired and orphaned workers receive terminal records. A parent
ending before collecting a child cancels the abandoned subtree. Cancellation
resolves live callers, closes approvals and unfinished tool cards, stops owned
plugin processes, clears task-local functions, and records the reason. The
ledger keeps the latest 128 records with bounded result excerpts. **Completed
means an answer was delivered, not that its claims have been verified**.

Esc/Stop cancels the whole tree. Quit, new chat and session replacement also
clean up managed work. **Stop resets the provider session**; the visible prior
transcript remains, but restate essential context before continuing. Partial
edits remain for review/undo: no automatic rollback and no silent replay of tool
side effects. Transport/model failover occurs before executing a completed tool
batch; agents may still independently choose to retry, so verify their work.

On Unix, managed commands, plugins and ACP agents get private process groups;
termination also stops ordinary descendants. Shell output capture is bounded.
A deliberately daemonizing program can escape, and non-Unix cleanup is
best-effort direct-child termination. A blocked model HTTP request may take up
to its 120-second transport bound to release its worker thread after Stop; the
cancellation flag prevents subsequent managed tool execution and late replies
are detached from a replacement session. The supervisor runs on the editor event
loop, not a hard-real-time independent watchdog.

All workers share the real workspace. Defaults are read-only; for writable
profiles, assign clear file ownership and check diffs. There are no isolated
worktrees, atomic multi-agent merges, or OS sandbox guarantees.

## Runtime function implementations

Agents may call `define_function` to assemble a reusable implementation from
pure operations. The built-in agent receives its function schema on the next
model round and can call the `fn_` name directly. MCP callers can always use
`call_function(name, arguments)`. `list_functions` discovers task-local schemas.

Example `define_function` arguments:

```json
{
  "name": "fn_unique_todos",
  "description": "Extract unique TODO messages from supplied text",
  "input_schema": {
    "type": "object",
    "properties": { "text": { "type": "string" } },
    "required": ["text"],
    "additionalProperties": false
  },
  "input": "/text",
  "steps": [
    { "op": "regex_extract", "pattern": "TODO:\\s*([^\\r\\n]+)", "group": 1 },
    { "op": "unique" },
    { "op": "sort" }
  ]
}
```

Then call `fn_unique_todos({"text":"TODO: add a test\nTODO: add a test"})`, or
`call_function({"name":"fn_unique_todos","arguments":{"text":"TODO: add a test"}})`.
The function transforms supplied data; reading a file or writing a result still
requires ordinary guarded host tools.

Supported operations: `trim`, `lowercase`, `uppercase`, `replace(from,to)`,
`split(separator)`, `join(separator)`, `lines`, `unique`, `sort`, `length`,
`json_parse`, `json_stringify`, `json_pointer(pointer)`, and
`regex_extract(pattern,group=0)`. JSON Pointers select values; regexes use Rust's
bounded regex engine, not backtracking/eval.

Limits: 16 functions per task, 16 KiB definition, 1–12 operations, 64 KiB serialized
input/output/intermediate values, 1024 items for array transformations such as
split/sort, 256 extracted
regex matches and bounded regex compilation. Names start with `fn_`, do not
replace existing functions and cannot shadow host tools. The supported schema
subset has typed objects/properties/required, arrays/items, scalar types/enum,
and string/array length bounds; unsupported keywords such as `$ref` are rejected.
Unknown input object properties are rejected by default.

Functions have no filesystem, network, shell, eval, loops, recursion or calls to
other tools. They are ephemeral: children receive private copies of their
parent's current registry, never sibling privileges; child additions do not
mutate a parent. They are cleared when the task/turn is cancelled or replaced,
and before a new root turn. This is a deliberately safe starting point for
agent-created implementations, **not** automatic Python/JavaScript execution.
Executable generated scripts still belong in the separately trusted,
user-approved command/plugin workflow.

## Verification and remaining checks

Run `cargo test --locked`, CI/lints and a real authenticated router task before
release. The automated integration fixtures use Python 3 and OpenSSL for local
mock providers and HTTPS; those tests do not verify the deployed router or an
actual account.
