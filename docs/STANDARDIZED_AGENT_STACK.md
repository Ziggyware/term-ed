# Standardized Agent Protocol Stack & Visual Architecture for term-ed

## 1. Protocol Matrix

| Layer | Protocol | Purpose in `term-ed` | Implementation |
|---|---|---|---|
| **Tools & Context** | **MCP** | Standardized access to files, AST search, diagnostics, and CLI runners. | JSON-RPC 2.0 (stdio/SSE/HTTP); `mcp-core`/`rmcp` |
| **Agent Delegation** | **A2A / ACP** | Agent-to-agent delegation (e.g. Architect → Coder → Auditor). Capability negotiation and handoffs. | JSON-RPC 2.0 (stdio/IPC); `termed-acp`/`termed-a2a` |
| **Frontend Streaming** | **AG-UI** | Real-time event stream from agent loops into graphical editors. | SSE/WebSockets; `termed-agui`, `axum`/`tokio-tungstenite` |
| **Declarative UI** | **A2UI** | Agent-generated interactive forms (permission dialogs, model pickers, parameter knobs). | JSON Schema v1.2; React/A2UI renderer |
| **Editor Mutations** | **AEP** | Multi-file unified diffs, fuzzy hunk application against `Ropey` buffers, patch rollback. | AEP JSON-Lines/Diffs; `termed-aep`, `similar`, `ropey` |
| **Typed Decisions** | **Jev** | Probabilistic typed judgments (`noul`, `choice`, `score`). Eliminates freeform prose parsing. | HTTP REST/JSON/FFI; `termed-decisions` (TypeSafe + Heuristic fallback) |

## 2. Jev Typed Decision Engine

Jev primitives — typed decisions with calibrated confidence scores:

1. **`noul`**: Binary judgment (yes/no/true/false) with confidence and margin.
   `ask_noul(query, context) -> (bool, confidence in [0.0, 1.0])`
2. **`choice`**: Selection from a closed set of explicit candidates.
   `ask_choice(query, [C1, ..., Ck], context) -> (Ci, distribution, confidence)`
3. **`score`**: Continuous calibrated probability between 0.0 and 1.0.
   `ask_score(query, context) -> (score in [0.0, 1.0], uncertainty)`

Backend trait (`crates/termed-decisions/src/lib.rs`): `ask_noul`, `ask_choice`, `ask_score` methods with `DecisionContext`, `NoulResult`, `ChoiceResult<T>`, and `ScoreResult`.

| Backend | Description |
|---|---|
| **LocalHeuristicBackend** (Default, Zero Config) | 100% offline; static rule tables and heuristic regexes (e.g. `rm`, `mkfs`, `sudo` yield risk ≥0.85; `git status`, `ls` yield risk <0.10). Works in Termux without internet. |
| **JevHttpBackend** (Opt-in) | Calls TypeSafe's Jev endpoints (`POST /v1/decisions/noul`, `POST /v1/decisions/score`). Sub-150ms calibrated probabilistic decisions. |
| **ReplayCassetteBackend** (Testing/CI) | Reads serialized decision cassettes (`tests/fixtures/cassettes/*.json`). Guarantees 100% deterministic test passes without external networks or stochastic LLMs. |

## 3. AG-UI Event Catalog

| Event | Direction | Payload Summary | Purpose |
|---|---|---|---|
| `run:initiated` | Agent → UI | `{ run_id, prompt, agent_graph_id }` | Run started; visual editor glows root agent node. |
| `agent:state_changed` | Agent → UI | `{ agent_id, state: "idle"\|"thinking"\|"executing" }` | Active agent pulse indicator. |
| `decision:evaluated` | Agent → UI | `{ query, result: NoulResult\|ScoreResult, backend }` | Visual badge showing Jev risk score or verdict. |
| `tool:started` | Agent → UI | `{ tool_id, tool_name, input_arguments }` | Animated edge from Agent node → Tool node. |
| `tool:completed` | Agent → UI | `{ tool_id, exit_code, output_preview, duration_ms }` | Tool output terminal display. |
| `patch:proposed` | Agent → UI | `{ file_path, diff_content, hunks, stats }` | Surfaces AEP diff viewer in visual canvas. |
| `patch:applied` | Agent → UI | `{ file_path, revision_id, success: bool }` | Updates editor buffer tabs and minimap. |
| `agent:delegate` | Agent → UI | `{ source_agent, target_agent, reason, context }` | Highlights A2A edge traversal in graph. |
| `ui:user_action` | UI → Agent | `{ action_type: "approve"\|"reject"\|"modify", data }` | User response from interactive A2UI component. |

## 4. A2UI Component Example

```json
{
  "a2ui_type": "decision_dialog",
  "id": "dialog_perm_412",
  "title": "Security Approval Required",
  "severity": "warning",
  "content": {
    "command": "git reset --hard HEAD~1",
    "jev_risk_score": 0.64,
    "jev_rationale": "Overwrites uncommitted working tree changes."
  },
  "actions": [
    { "id": "allow_once", "label": "Allow Once", "style": "primary", "shortcut": "y" },
    { "id": "deny", "label": "Deny", "style": "destructive", "shortcut": "n" },
    { "id": "inspect_diff", "label": "View Diff First", "style": "secondary", "shortcut": "d" }
  ]
}
```

## 5. AEP Patch Example

JSON-Lines patch format:

```json
{"op":"add","path":"src/editor.rs","hunk":[300,310,"old text","new text"]}
{"op":"remove","path":"src/editor.rs","hunk":[150,160,"",""]}
```

Hunks: structured ranges `[start_line, end_line, old_text, new_text]`. Patch operations commit to a `PatchTransaction` stack enabling single-keystroke undo/redo across multi-file edits.

## 6. Target Rust Workspace Architecture

```
term-ed/
├── Cargo.toml                  # Virtual workspace root
├── crates/
│   ├── termed-core/            # Buffer management (Ropey), syntax (Syntect), theme, config
│   ├── termed-tui/             # Ratatui terminal UI rendering & input loop
│   ├── termed-mcp/             # MCP client & server implementations (tools & context)
│   ├── termed-a2a/             # ACP / A2A multi-agent communication & routing
│   ├── termed-decisions/       # Jev typed decision engine (Noul, Choice, Score)
│   ├── termed-aep/             # Diff/patch engine, hunk applicator, transaction log
│   └── termed-agui/            # AG-UI SSE & WebSocket event streaming server
├── companion-ui/               # Visual Web UI Editor (React + React Flow + CopilotKit)
│   ├── src/components/GraphCanvas.tsx
│   ├── hooks/useAGUIStream.ts
│   └── schemas/graph.ts
└── tests/
    └── cassettes/              # Recorded Jev & agent interaction fixtures
```
