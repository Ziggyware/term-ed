# Settings, plugins, agents and skills

## Menus

- **Settings** in the desktop title bar, **Preferences: Settings** in the command palette, or **Ctrl+,** opens user preferences.
- **Plugins** in the title bar, **Plugins: Manage Plugins**, or **Ctrl+Alt+P** opens plugin management. Select a plugin for its commands, enable/disable switch, AI permission and manifest.
- **Agent: Configure AI Agent**, **Agent: Manage Sub-agents**, and **Skills: Manage Skills** go directly to those settings. The agent panel's **⚙** button opens agent configuration.
- On a phone, use **⋯** to find any of these commands. Menus are searchable and support arrows, Enter, Esc and touch. Setting values are preselected; type or paste to replace, **Ctrl+A** selects all, and **Ctrl+Backspace** clears the field.

## User settings and migration

Menu changes save a complete user configuration at **`$XDG_CONFIG_HOME/termed/settings.json`** (normally `~/.config/termed/settings.json`). `XDG_CONFIG_HOME` must be an absolute path; an empty or relative value is ignored. On Windows, when `XDG_CONFIG_HOME` and `HOME` are unset, it is under `%USERPROFILE%\.config\termed\settings.json`. The same settings load across folders and restarts; opening the editor alone does not create a file. On Unix, a newly created configuration directory is private (`0700`), and settings are atomically replaced using a private (`0600`) file. Other platforms rely on their user-directory ACLs. The separate **Save Router API Key** action stores credentials in a user-local credentials file, never in settings JSON. See [router setup and supervision](router-and-supervision.md).

For migration, if no user settings file exists, termed reads the current folder's old `.termed/settings.json`. Version 1 or unversioned automatic/OpenCode backends are migrated to the built-in agent; the old implicit built-in gateway becomes the router, and an old OpenCode default model is cleared. A deliberately selected `auto`, `kilo`, or custom provider in version 2 is retained. Other settings, including explicitly selected non-OpenCode backends, custom providers and permission restrictions, are preserved. Loading alone does not write a file. The first successful menu save writes these settings at user scope and leaves the old workspace file untouched.

**Important:** Once a user settings file exists it is authoritative, and legacy `.termed/settings.json` files in *all* folders are ignored—including any per-project instructions, `read_only`, command or plugin restrictions. An in-editor notice appears when a legacy file is present. Before migrating, inspect and copy any restrictions you need into the user settings; reapply folder-specific safeguards yourself. The old workspace files are not deleted. Skills and plugins still have their own workspace discovery rules.

Use **Open User Settings JSON** for multi-line instructions or bulk edits, then **Reload Settings from Disk**. Invalid JSON, unsupported versions, invalid budgets and unavailable backends are reported; the last live configuration is retained on a failed reload. At startup, an invalid configuration falls back to defaults with an error notification. Menu changes do not overwrite an invalid configuration or an unsaved user settings buffer. Stop an AI turn before changing its agent/sub-agent configuration.

Editor preferences include theme, minimap, whitespace, indent guides, line numbers, word wrap and the touch toolbar. Wrap and toolbar each cycle through automatic/on/off. Editor display commands also persist their changes.

## Configuring the primary agent

The agent configuration menu exposes:

| Setting | Meaning |
| --- | --- |
| Backend | `builtin` (default), `custom`, or the exact display name of an installed ACP agent; legacy `auto` selects the built-in agent |
| Custom ACP command | An argv string with single/double-quoted arguments; no shell expansion. Set it before choosing `custom` |
| Model | Explicit provider model ID; empty preserves the environment/provider default |
| API base URL | Built-in agent's OpenAI-compatible base URL, including `/v1` when required; empty uses the selected provider. The default router uses `https://nokey-router.ziggyware.deno.net/v1` |
| API key environment variable | **The name of an environment variable**, never the API key itself |
| Instructions | Role, conventions and additional instructions; use JSON for multiple lines |
| Step budget | 1–100 built-in model/tool rounds per turn (default 30) |
| Read-only | Built-in agent cannot edit/write files, run commands or execute plugins |
| Command permissions | `ask` (default), `deny`, or `risk_assessed` |
| AI plugin tools | Whether plugin discovery and execution tools are exposed |
| Plugin allowlist | Comma-separated names; empty allows all enabled plugins except explicit plugin-menu denials |
| Attached skills | Comma-separated enabled skill IDs to include on each prompt |
| Sub-agent delegation | Whether the primary AI can delegate to configured specialists |
| Delegations per turn | 1–32 total tasks for the whole root tree (default 4); descendants share this budget |
| Task timeout | 10–1800 seconds (default 300) for the root turn and inherited absolute specialist deadlines, including approval waits |
| Attach editor context | Include the active file and selection in prompts and bounded worker briefs |
| Built-in provider | `router` (default), `auto` (legacy Kilo/custom), `kilo`, `custom`; external ACP processes configure their own providers |
| Maximum agent depth | 1–3 (default 2): root=0, child=1, grandchild=2; every ancestor can impose a smaller cap |
| Concurrent tasks | 1–8 active tasks (default 4), including waiting coordinators; new delegation fails fast when full |
| Overwatch stall timeout | 15–600 seconds (default 150); child/approval/plugin waits are exempt from idle detection, not absolute deadlines |
| Pure generated functions | Enable bounded task-scoped data transformations; no generated code execution |
| Model output token budget | 256–16384 (default 4096), inherited restrictively |

Explicit settings take precedence over environment defaults. Custom provider requests use `Authorization: Bearer <value of api_key_env>`; missing credentials are reported without exposing their value. Custom endpoints use the explicitly chosen model rather than discovering/failing over to unrelated free models.

With `ask`, each shell command asks permission unless you explicitly select the session's Always Allow option. `deny` blocks shell commands. `risk_assessed` allows the existing risk policy to auto-allow low-risk commands. High-risk commands remain blocked in all modes. Plugin approval is separate and **always per invocation**.

External ACP agents receive configured instructions, selected skills, model selection and an MCP server definition. Provider URLs, built-in step budgets and built-in tool restrictions do not reconfigure or sandbox an external process. An external agent must support ACP MCP servers to use the live editor tools. Use the built-in agent for host-enforced read-only and command restrictions; external agents and plugins remain trusted local programs with your account's filesystem access.

### Example

This is a partial configuration; omitted fields retain defaults:

```json
{
  "version": 2,
  "editor": { "minimap": false, "word_wrap": true },
  "agent": {
    "backend": "builtin",
    "provider": "custom",
    "endpoint": "http://127.0.0.1:11434/v1",
    "model": "your-tool-capable-model",
    "api_key_env": "",
    "instructions": "Prefer small patches. Explain how to verify each change.",
    "max_steps": 20,
    "commands": "ask",
    "plugins": true,
    "allowed_plugins": ["text-tools", "stats", "todo"],
    "denied_plugins": [],
    "skills": ["project-style"],
    "sub_agents": true,
    "max_delegations": 3
  }
}
```

The loopback URL above is for a model server on the **same machine as termed**, not a browser-facing service. A remote provider's HTTPS API URL works too. Use **API key environment variable** for authentication rather than putting secrets into JSON or URLs.

## AI plugin tools

The built-in agent and external MCP clients share these live tools:

- `read_editor(file?)`: read an open buffer, including unsaved plugin edits.
- `list_plugins()`: enabled, runnable, permitted plugins and their exact command IDs.
- `run_plugin(plugin, command, arguments?, file?)`: request approval, run the exact command and wait for its real JSON-RPC result. `arguments` is passed as `params.arguments` to the plugin.
- `list_skills()` / `read_skill(id)`.
- `list_sub_agents()` / `delegate_task(agent, task, acceptance_criteria?, constraints?, context?, context_files?)`.
- `report_progress(message)`, `task_status()`, `cancel_task(task_id)`.
- `define_function(...)`, `list_functions()`, `call_function(name, arguments)`; built-in agents also get each generated `fn_` schema dynamically.

Disabled tools are omitted from schemas **and** rejected at dispatch. Plugin allowlists and explicit denials also apply to delegated workers. Selecting Reject or stopping the turn returns an error to the AI; it does not launch the plugin command. A plugin that exits, times out, reports a JSON-RPC error or emits an error toast is reported as a failure, not a successful process launch.

Plugin calls target the explicit workspace file or the active buffer captured when approval was requested. Switching tabs while a plugin is running does not redirect its edits. Changes remain unsaved and undoable, just like manual plugin commands. Results include plugin messages and the resulting buffer (with a size limit). **Read the live buffer after plugin edits; disk reads can still contain the old saved version.** Executable plugins are not sandboxed: inspect their manifests/scripts before enabling them or approving execution.

Plugin command execution times out after 120 seconds; unanswered plugin approvals expire after five minutes. Stop cancels pending approvals, stops active plugin processes and cancels delegated tasks. Partial plugin edits remain visible and undoable rather than being silently reverted.

### External ACP / MCP transport

For external agents, termed supplies an authenticated **stdio MCP relay** in ACP's `mcpServers` list. It forwards requests to the live editor through a randomly authenticated loopback connection. Tools operate on the same buffers and approval cards as the built-in agent. The listener is not exposed on the network, the authentication value is session-local, and the relay writes JSON-RPC only to stdout. `--mcp-bridge` is an internal command, not a public unauthenticated tool server.

## Sub-agents

Three editable profiles are provided by default: **Reviewer**, **Test planner**, and **Coordinator**. All are read-only; the coordinator may delegate to the leaf profiles. Existing saved profile arrays remain authoritative. The sub-agent menu supports creating, enabling/disabling, changing the name/description, model, provider, instructions, budgets, permissions and attached skills, and deleting a profile with confirmation.

Sub-agents are **built-in specialists**, even when the primary agent is external. Each gets a separate conversation, a scoped task brief, inherited project/ancestor instructions, immutable inherited skill snapshots and its own role/additional skills. An empty model and `auto` provider inherit the primary configuration. An inherited API key environment variable is not forwarded to a different explicitly configured provider endpoint.

The host intersects permissions with the parent's settings: workers cannot bypass read-only mode, command denial, plugin restrictions, step limits or the task deadline. Nested delegation is bounded by depth, lineage cycle checks, global concurrency/delegation caps and inherited absolute deadlines. `sub_agents` must be enabled on a coordinator profile, not just the root. Results return to the parent as tool output; worker tool activity and approval requests are visible in the main transcript. Esc/Stop cancels the entire task tree. This is bounded task delegation, not a new operating-system sandbox or an autonomous always-running agent service.

## Skills

A skill is a folder containing **`SKILL.md`**, not an executable:

```text
<workspace>/.termed/skills/project-style/SKILL.md
~/.config/termed/skills/project-style/SKILL.md
```

Workspace skills override user skills with the same ID. IDs contain 1–64 ASCII letters, digits, `-` or `_`. Skills are limited to 32 KB each and discovery is bounded. Simple single-line `name:` and `description:` front matter is supported:

```markdown
---
name: Project style
description: Coding conventions and verification steps for this project
---

# Project style

Prefer small, focused changes. Add regression tests for Unicode and empty input.
Run the project's relevant tests before reporting completion.
```

**Skills: Manage Skills** can create a skill, open its instructions in the editor, enable/disable it, attach/detach it from the primary agent, reload discovery and delete a workspace skill with confirmation. Per-specialist attached skill IDs are editable in each sub-agent's configuration. Global skills can be edited/disabled here, but not deleted by the workspace menu. Deletion removes `SKILL.md` only, not arbitrary additional files in its folder, and refuses to discard dirty instruction buffers.

Required attached skills must exist, be enabled/readable and fit a combined 96 KiB budget, or the task fails explicitly instead of silently omitting instructions. Skill snapshots are inherited by descendants; other enabled skills are available on demand through the skill tools. Disabled skills are not listed, read through `read_skill`, or attached automatically. Disabling does not redact instructions already present in a conversation; start a new chat to clear prior context. Skills contain instructions sent to your configured model provider, so do not include secrets.

## Verification

Regression coverage lives in `tests/search.rs`, `tests/management.rs`, `tests/automation.rs` and the automation module's unit tests. `mock_automation_gateway.py` exercises plugin approval, unsaved buffer updates, skill loading and a read-only specialist using a local deterministic provider, without API keys or external model access.

```sh
cargo test
cargo clippy --all-targets
```

Integration fixtures require Python 3; HTTPS fixtures also use OpenSSL. Run the production locked-dependency build and tests on the intended platforms before release. A live authenticated router request is a separate check from the local mock tests.
