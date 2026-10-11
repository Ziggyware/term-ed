# Standardized Agent Protocol Stack & Visual Architecture for term-ed
**Architecture Blueprint: MCP + A2A + AG-UI + A2UI + AEP + Jev Decision Engine**
*Target Repository: [Ziggyware/term-ed](https://github.com/Ziggyware/term-ed.git)*  
*Edition: 2026 Production Specification*

---

## Executive Summary

`term-ed` began as a fast, pure-Rust terminal editor (Ratatui, Ropey, Syntect) equipped with an embedded Agent Client Protocol (ACP) client and an in-process free-tier AI coding agent. As agentic development expands into multi-agent workflows, autonomous refactoring, and visual graph editing, ad-hoc JSON-RPC bridges and prompt-parsing heuristics become bottlenecks.

This document establishes the **standardized set of libraries, protocols, and data schemas** required to evolve `term-ed` into an interoperable, visually inspectable, and reliably governed agent platform:

1. **Model Context Protocol (MCP)**: Unifies tools, file system access, terminal execution, and LSP context.
2. **Agent-to-Agent (A2A / ACP)**: Governs delegation, sub-agent orchestration, and task handoffs.
3. **Agent-to-UI Streaming (AG-UI)**: Powers real-time event streaming (SSE/WebSocket) to visual editors and web runtimes.
4. **Agent User Interface (A2UI)**: Declarative JSON UI schema for agent-rendered interaction components.
5. **Agent Editing Protocol (AEP)**: Standardized structured diffs, hunks, atomic transactions, and undo/redo trees.
6. **Jev Typed Decision Engine (`decisions` crate)**: Replaces brittle LLM prose parsing with typed queries (`noul`, `choice`, `score`), powering intelligent permission gating, model failover, and edit verification.

---

## 1. The Standardized Protocol Matrix

The modern agent stack cleanly separates context, delegation, UI streaming, component declaration, file mutations, and discrete decision-making:

```
+---------------------------------------------------------------------------------------+
|                                    VISUAL UI RUNTIME                                  |
|   (React / CopilotKit / LangGraph Studio / assistant-ui / term-ed Web Companion)       |
+---------------------------------------------------------------------------------------+
        ^                                 ^                                 ^
        | AG-UI Event Stream              | A2UI Declarative Components     | AEP Diff/Patches
        v                                 v                                 v
+---------------------------------------------------------------------------------------+
|                                   term-ed HOST RUNTIME                                |
|                                                                                       |
|   +-----------------------+     A2A / ACP     +-----------------------------------+   |
|   |    Architect Agent    | <---------------> |   Coder / Worker Agent (termed AI)|   |
|   +-----------------------+                   +-----------------------------------+   |
|               |                                                 |                     |
|               | Decides routing & risk                          | Verifies changes    |
|               v                                                 v                     |
|   +-------------------------------------------------------------------------------+   |
|   |                   Jev Decision Engine (`termed-decisions`)                     |   |
|   |         [ ask_noul (yes/no) | ask_choice (closed set) | ask_score (0-1) ]      |   |
|   |         Backends: 1. Jev Cloud API  2. Local Heuristic  3. Replay Cassette    |   |
|   +-------------------------------------------------------------------------------+   |
|                                         |                                             |
|                                         | Executes tools                              |
|                                         v                                             |
|   +-------------------------------------------------------------------------------+   |
|   |                         Model Context Protocol (MCP)                          |   |
|   |         File System | LSP Diagnostics | Shell Runner | Git Connector          |   |
|   +-------------------------------------------------------------------------------+   |
+---------------------------------------------------------------------------------------+
```

### Detailed Protocol Taxonomy

| Layer | Protocol | Purpose in `term-ed` | Wire Transport | Rust / Ecosystem Implementation |
|---|---|---|---|---|
| **Tools & Context** | **MCP** | Standardized access to files, AST search, diagnostics, and CLI runners. Replaces ad-hoc builtin tool calls. | JSON-RPC 2.0 (stdio / SSE / HTTP) | `mcp-core` / `rmcp` |
| **Agent Delegation** | **A2A / ACP** | Agent-to-agent delegation (e.g. Architect $\rightarrow$ Coder $\rightarrow$ Auditor). Capability negotiation and handoffs. | JSON-RPC 2.0 (stdio / IPC / in-process channels) | `termed-acp`, `termed-a2a` |
| **Frontend Streaming** | **AG-UI** | Real-time event stream from agent execution loops into graphical web editors and browser companion views. | Server-Sent Events (SSE) / WebSockets | `termed-agui`, `axum` / `tokio-tungstenite` |
| **Declarative UI** | **A2UI** | Agent-generated interactive forms (permission dialogs, model pickers, parameter knobs). | JSON Spec (Schema v1.2) | `a2ui-core`, React/A2UI renderer |
| **Editor Mutations** | **AEP** | Multi-file unified diffs, fuzzy hunk application against `Ropey` buffers, patch rollback. | AEP JSON-Lines / Patch Diffs | `termed-aep`, `similar`, `ropey` |
| **Typed Decisions** | **Jev** | Probabilistic typed judgments (`noul`, `choice`, `score`). Eliminates freeform prose parsing. | HTTP REST / JSON / In-memory FFI | `termed-decisions` (TypeSafe backend + Heuristic fallback) |

---

## 2. Jev Typed Decision Engine (`termed-decisions`)

### 2.1 The Problem: Prose Parsing Fragility

Current agent implementations parse freeform LLM prose into JSON or regexes to answer critical questions:
- *"Is this bash command dangerous?"*
- *"Has model X timed out, and which backup model should we switch to?"*
- *"Did this file edit satisfy the user's instructions?"*

This leads to prompt injection vulnerabilities, JSON formatting failures, high latency, and indeterminate behavior.

### 2.2 The Solution: Jev Primitives

Jev (TypeSafe / `typesafe.ai`) is a typed decision engine. The client application enforces the finite state machine and asks typed questions with probabilistic outputs and calibrated confidence scores:

1. **`noul`**: Binary judgment (yes/no/true/false) with confidence and margin.
   $$\text{ask\_noul}(\text{query}, \text{context}) \rightarrow (\text{bool}, \text{confidence} \in [0.0, 1.0])$$
2. **`choice`**: Selection from a closed set of explicit candidates.
   $$\text{ask\_choice}(\text{query}, [C_1, C_2, \dots, C_k], \text{context}) \rightarrow (C_i, \text{distribution}, \text{confidence})$$
3. **`score`**: Continuous calibrated probability between 0.0 and 1.0.
   $$\text{ask\_score}(\text{query}, \text{context}) \rightarrow (\text{score} \in [0.0, 1.0], \text{uncertainty})$$

---

### 2.3 Concrete Use Cases in `term-ed`

#### 1. Tiered Permission Gating (Replacing Blunt `run_command` Dialogs)
* **Current status in `term-ed`**: `builtin_agent.rs` prompts the user for *every* shell command, regardless of whether it is `ls -la` or `rm -rf /`.
* **With Jev `ask_score`**: Command risk is evaluated on a continuous scale $[0.0, 1.0]$:
  - **Score $< 0.20$** (Safe reads: `git status`, `cargo check`, `find`): **Auto-allowed** (silent execution, logged to AG-UI).
  - **$0.20 \le \text{Score} < 0.75$** (File writes, dependency additions): **Interactive prompt** in TUI/GUI with one-click approval.
  - **Score $\ge 0.75$** (Destructive: `rm -rf`, force push, credentials access): **Hard Block** with explanation, requiring explicit security override.

#### 2. Dynamic Model Selection & Failover
* **Current status in `term-ed`**: Rigid 25-second stall timer (`stall_after: Duration::from_secs(25)`). If silent, it switches models blindly.
* **With Jev `ask_choice`**:
  - When latency spikes, rate limits occur, or prompt complexity shifts, Jev evaluates the candidate pool (`["kilo-auto/free", "gemini-2.5-pro", "claude-3-7-sonnet", "local-ollama"]`) against token budget, required reasoning depth, and agent responsiveness.

#### 3. Tool-Output & Edit Judging (`jev-judge`)
* **Current status in `term-ed`**: Agent applies edits blindly and hopes the output matches user intent.
* **With Jev `ask_noul`**:
  - After an AEP patch is applied, `jev-judge` executes a fast verification pass:
    - Did the diff introduce syntax errors?
    - Does `cargo check` clean up?
    - Does the diff fulfill the original user prompt?
  - Returns `judgment: true/false` + confidence. If `false`, triggers an automatic self-repair cycle before notifying the user.

---

### 2.4 Architecture of the `termed-decisions` Crate

To maintain `term-ed`'s core philosophy—**pure Rust, zero mandatory external accounts, works offline in Termux**—the decision engine is built around a pluggable backend trait:

```rust
// crates/termed-decisions/src/lib.rs

use serde::{Deserialize, Serialize};
use std::fmt::Debug;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionContext {
    pub task_id: String,
    pub working_dir: String,
    pub session_history_summary: String,
    pub metadata: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoulResult {
    pub judgment: bool,
    pub confidence: f32, // 0.0 to 1.0
    pub margin: f32,
    pub reasoning_summary: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChoiceResult<T: Clone + Debug> {
    pub selected: T,
    pub index: usize,
    pub probabilities: Vec<f32>,
    pub confidence: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoreResult {
    pub score: f32,       // 0.0 (safe/low) to 1.0 (dangerous/high)
    pub uncertainty: f32, // Epistemic uncertainty
    pub flags: Vec<String>,
}

pub trait DecisionEngine: Send + Sync {
    /// Binary evaluation (True / False)
    fn ask_noul(&self, question: &str, ctx: &DecisionContext) -> anyhow::Result<NoulResult>;

    /// Discrete choice over legal candidates
    fn ask_choice(
        &self,
        question: &str,
        candidates: &[String],
        ctx: &DecisionContext,
    ) -> anyhow::Result<ChoiceResult<String>>;

    /// Continuous calibrated metric [0.0, 1.0]
    fn ask_score(
        &self,
        metric_name: &str,
        target_payload: &str,
        ctx: &DecisionContext,
    ) -> anyhow::Result<ScoreResult>;
}
```

#### The Three Engine Backends

```
                              +-----------------------+
                              |   <<DecisionEngine>>  |
                              |         Trait         |
                              +-----------------------+
                                          |
            +-----------------------------+-----------------------------+
            |                             |                             |
            v                             v                             v
+-----------------------+     +-----------------------+     +-----------------------+
|    JevHttpBackend     |     | LocalHeuristicBackend |     | ReplayCassetteBackend |
+-----------------------+     +-----------------------+     +-----------------------+
| - Cloud TypeSafe API  |     | - 100% Offline / Free |     | - Deterministic replay|
| - Calibrated probas   |     | - Regex & AST rules   |     | - Recorded test runs  |
| - High-reasoning tier |     | - Default in termed   |     | - CI/CD regression    |
+-----------------------+     +-----------------------+     +-----------------------+
```

1. **`LocalHeuristicBackend` (Default, Zero Config)**:
   - Evaluates commands against static rule tables, path restrictions, and heuristic regexes (e.g. `rm`, `mkfs`, `sudo`, `curl | sh` yield risk $\ge 0.85$; `git status`, `ls` yield risk $< 0.10$).
   - Completely free, works in Termux without internet access.
2. **`JevHttpBackend` (Opt-in via API Key / `TERMED_JEV_API_KEY`)**:
   - Calls TypeSafe's Jev endpoints (`POST /v1/decisions/noul`, `POST /v1/decisions/score`).
   - Delivers sub-150ms calibrated probabilistic decisions trained specifically on agent governance.
3. **`ReplayCassetteBackend` (Automated Testing & CI)**:
   - Reads serialized decision cassettes (`tests/fixtures/cassettes/*.json`).
   - Guarantees 100% deterministic test passes across CI runs without invoking external networks or stochastic LLMs.

---

## 3. Real-Time Frontend Streaming Protocol: AG-UI

The **AG-UI protocol** defines a real-time event streaming standard (over Server-Sent Events or WebSockets) connecting agent runtimes to graphical frontend editors.

### 3.1 AG-UI Event Wire Format

Every AG-UI event is a typed JSON frame:

```json
{
  "event_id": "evt_01JAD4K7W598V12X",
  "version": "1.0",
  "timestamp": 1775923200150,
  "session_id": "sess_term_ed_4091",
  "type": "tool:started",
  "payload": {
    "agent_id": "coder_agent_01",
    "tool_id": "call_mcp_fs_write",
    "tool_name": "mcp::filesystem::write_file",
    "params": {
      "path": "src/editor.rs",
      "hunk_count": 2
    }
  }
}
```

### 3.2 Complete Event Catalog

| Event Name | Direction | Payload Schema Summary | Purpose |
|---|---|---|---|
| `run:initiated` | Agent $\rightarrow$ UI | `{ run_id, prompt, agent_graph_id }` | Run started; visual editor glows root agent node. |
| `agent:state_changed` | Agent $\rightarrow$ UI | `{ agent_id, state: "idle"\|"thinking"\|"executing" }` | Active agent pulse indicator. |
| `decision:evaluated` | Agent $\rightarrow$ UI | `{ query, result: NoulResult\|ScoreResult, backend }` | Visual badge showing Jev risk score or verdict. |
| `tool:started` | Agent $\rightarrow$ UI | `{ tool_id, tool_name, input_arguments }` | Animated edge from Agent node $\rightarrow$ Tool node. |
| `tool:completed` | Agent $\rightarrow$ UI | `{ tool_id, exit_code, output_preview, duration_ms }` | Tool output terminal display. |
| `patch:proposed` | Agent $\rightarrow$ UI | `{ file_path, diff_content, hunks, stats }` | Surfaces AEP diff viewer in visual canvas. |
| `patch:applied` | Agent $\rightarrow$ UI | `{ file_path, revision_id, success: bool }` | Updates editor buffer tabs and minimap. |
| `agent:delegate` | Agent $\rightarrow$ UI | `{ source_agent, target_agent, reason, context }` | Highlights A2A edge traversal in graph. |
| `ui:user_action` | UI $\rightarrow$ Agent | `{ action_type: "approve"\|"reject"\|"modify", data }` | User response from interactive A2UI component. |

---

## 4. Visual Routing Graph: Schema & Editor Architecture

To visualize and edit multi-agent routing, tools, and permission policies, the frontend editor (built on CopilotKit, React Flow, or LangGraph Studio) persists and loads a declarative **Agent Routing Graph**.

### 4.1 JSON Graph Schema (`term-ed-routing-graph.schema.json`)

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "TermEdAgentRoutingGraph",
  "type": "object",
  "required": ["version", "graph_id", "nodes", "edges"],
  "properties": {
    "version": { "type": "string", "enum": ["2026-04"] },
    "graph_id": { "type": "string" },
    "name": { "type": "string" },
    "nodes": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["id", "type", "label", "position"],
        "properties": {
          "id": { "type": "string" },
          "type": {
            "type": "string",
            "enum": ["agent", "tool", "decision_gate", "data_source", "ui_sink"]
          },
          "label": { "type": "string" },
          "position": {
            "type": "object",
            "required": ["x", "y"],
            "properties": {
              "x": { "type": "number" },
              "y": { "type": "number" }
            }
          },
          "config": {
            "type": "object",
            "properties": {
              "provider": { "type": "string" },
              "model": { "type": "string" },
              "decision_primitive": { "type": "string", "enum": ["noul", "choice", "score"] },
              "thresholds": {
                "type": "object",
                "properties": {
                  "auto_allow": { "type": "number" },
                  "hard_block": { "type": "number" }
                }
              }
            }
          }
        }
      }
    },
    "edges": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["id", "source", "target", "protocol"],
        "properties": {
          "id": { "type": "string" },
          "source": { "type": "string" },
          "target": { "type": "string" },
          "protocol": {
            "type": "string",
            "enum": ["mcp", "a2a", "ag_ui", "aep", "typed_decision"]
          },
          "routing_condition": { "type": "string" }
        }
      }
    }
  }
}
```

### 4.2 Example Concrete Routing Graph

```json
{
  "version": "2026-04",
  "graph_id": "termed_standard_pipeline",
  "name": "term-ed Standard Autonomous Engineering Workflow",
  "nodes": [
    {
      "id": "node_orchestrator",
      "type": "agent",
      "label": "Architect Agent",
      "position": { "x": 100, "y": 200 },
      "config": { "model": "kilo-auto/free", "role": "planner" }
    },
    {
      "id": "node_coder",
      "type": "agent",
      "label": "Coder Agent (termed AI)",
      "position": { "x": 400, "y": 150 },
      "config": { "model": "kilo-auto/free", "role": "code_modifier" }
    },
    {
      "id": "node_perm_gate",
      "type": "decision_gate",
      "label": "Jev Risk Gate",
      "position": { "x": 400, "y": 320 },
      "config": {
        "decision_primitive": "score",
        "thresholds": { "auto_allow": 0.20, "hard_block": 0.75 }
      }
    },
    {
      "id": "node_mcp_fs",
      "type": "tool",
      "label": "MCP Workspace Tools",
      "position": { "x": 700, "y": 150 },
      "config": { "provider": "mcp", "tools": ["read_file", "edit_file", "search"] }
    },
    {
      "id": "node_mcp_sh",
      "type": "tool",
      "label": "MCP Shell Runner",
      "position": { "x": 700, "y": 320 },
      "config": { "provider": "mcp", "tools": ["run_command"] }
    },
    {
      "id": "node_reviewer",
      "type": "agent",
      "label": "Jev-Judge Reviewer",
      "position": { "x": 400, "y": 480 },
      "config": { "decision_primitive": "noul", "role": "verifier" }
    }
  ],
  "edges": [
    {
      "id": "edge_orch_coder",
      "source": "node_orchestrator",
      "target": "node_coder",
      "protocol": "a2a"
    },
    {
      "id": "edge_coder_fs",
      "source": "node_coder",
      "target": "node_mcp_fs",
      "protocol": "mcp"
    },
    {
      "id": "edge_coder_gate",
      "source": "node_coder",
      "target": "node_perm_gate",
      "protocol": "typed_decision"
    },
    {
      "id": "edge_gate_exec",
      "source": "node_perm_gate",
      "target": "node_mcp_sh",
      "protocol": "mcp",
      "routing_condition": "score < 0.75"
    },
    {
      "id": "edge_coder_review",
      "source": "node_coder",
      "target": "node_reviewer",
      "protocol": "a2a"
    }
  ]
}
```

---

## 5. Declarative UI (A2UI) & Patch Engine (AEP)

### 5.1 A2UI: Declarative Interactive Components

When an agent needs user feedback (e.g. permission prompt for commands with score $0.20 \le s < 0.75$, or selecting disambiguated target files), it emits an **A2UI JSON Component Spec**. Both the Ratatui TUI and the AG-UI web frontend render this deterministically:

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

### 5.2 AEP: Agent Editing Protocol

AEP standardizes how code modifications stream between the agent and `term-ed`'s `Ropey` text buffer:
1. **Patch Streaming**: Hunks are sent as structured ranges `[start_line, end_line, old_text, new_text]`.
2. **Fuzzy Buffer Reconciliation**: Uses `nucleo-matcher` and `similar` to match lines even if user edits occurred concurrently.
3. **Atomic Reversibility**: Each patch operation is committed to a `PatchTransaction` stack, enabling single-keystroke undo/redo across multi-file edits.

---

## 6. Target Rust Workspace Architecture for `term-ed`

To implement this modular standard, `term-ed` is structured as a high-cohesion Cargo workspace:

```
term-ed/
├── Cargo.toml                  # Virtual workspace root
├── crates/
│   ├── termed-core/            # Buffer management (Ropey), syntax (Syntect), theme, config
│   ├── termed-tui/             # Ratatui terminal UI rendering & input loop
│   ├── termed-mcp/             # MCP client & server implementations (tools & context)
│   ├── termed-a2a/             # ACP / A2A multi-agent communication & routing
│   ├── termed-decisions/       # Jev typed decision engine (Noul, Choice, Score)
│   │   ├── src/
│   │   │   ├── engine.rs       # DecisionEngine trait
│   │   │   ├── jev_http.rs     # TypeSafe Jev Cloud client
│   │   │   ├── heuristic.rs    # Pure offline rule-based backend
│   │   │   └── cassette.rs     # Deterministic replay backend
│   ├── termed-aep/             # Diff/patch engine, hunk applicator, transaction log
│   └── termed-agui/            # AG-UI SSE & WebSocket event streaming server
├── companion-ui/               # Visual Web UI Editor (React + React Flow + CopilotKit)
│   ├── src/
│   │   ├── components/GraphCanvas.tsx
│   │   ├── hooks/useAGUIStream.ts
│   │   └── schemas/graph.ts
└── tests/
    └── cassettes/              # Recorded Jev & agent interaction fixtures
```

---

## 7. Concrete Code Implementation Examples

### 7.1 Integrating Jev Permission Gating into `run_command`

Replacing the blunt prompt in `src/builtin_agent.rs`:

```rust
// In crates/termed-decisions/src/permission.rs

use crate::engine::{DecisionContext, DecisionEngine, ScoreResult};
use std::sync::Arc;

pub enum PermissionVerdict {
    AutoAllow,
    PromptUser { score: f32, rationale: String },
    Block { score: f32, reason: String },
}

pub struct PermissionPolicy {
    engine: Arc<dyn DecisionEngine>,
    auto_allow_threshold: f32, // default: 0.20
    block_threshold: f32,      // default: 0.75
}

impl PermissionPolicy {
    pub fn new(engine: Arc<dyn DecisionEngine>) -> Self {
        Self {
            engine,
            auto_allow_threshold: 0.20,
            block_threshold: 0.75,
        }
    }

    pub fn evaluate_command(&self, command: &str, cwd: &str) -> PermissionVerdict {
        let ctx = DecisionContext {
            task_id: "cmd_eval".into(),
            working_dir: cwd.into(),
            session_history_summary: String::new(),
            metadata: serde_json::json!({ "command": command }),
        };

        let result = self.engine.ask_score("shell_execution_risk", command, &ctx)
            .unwrap_or(ScoreResult {
                score: 0.50, // default conservative fallback
                uncertainty: 0.50,
                flags: vec!["engine_fallback".into()],
            });

        if result.score < self.auto_allow_threshold {
            PermissionVerdict::AutoAllow
        } else if result.score >= self.block_threshold {
            PermissionVerdict::Block {
                score: result.score,
                reason: format!("High risk execution blocked (risk score: {:.2})", result.score),
            }
        } else {
            PermissionVerdict::PromptUser {
                score: result.score,
                rationale: format!("Command requires confirmation (risk score: {:.2})", result.score),
            }
        }
    }
}
```

### 7.2 Implementing the AG-UI Event Hub

```rust
// In crates/termed-agui/src/hub.rs

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AGUIEvent {
    pub event_id: String,
    pub timestamp: u64,
    pub session_id: String,
    pub event_type: String,
    pub payload: serde_json::Value,
}

#[derive(Clone)]
pub struct AGUIHub {
    tx: broadcast::Sender<AGUIEvent>,
}

impl AGUIHub {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn emit(&self, event_type: &str, session_id: &str, payload: serde_json::Value) {
        let event = AGUIEvent {
            event_id: format!("evt_{}", uuid::Uuid::new_v4().simple()),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            session_id: session_id.to_string(),
            event_type: event_type.to_string(),
            payload,
        };
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AGUIEvent> {
        self.tx.subscribe()
    }
}
```

---

## 8. Development Roadmap & Implementation Steps

| Phase | Milestone | Deliverables | Verification Criteria |
|---|---|---|---|
| **Phase 1** | **`termed-decisions` Crate** | Trait definitions, `LocalHeuristicBackend`, `ReplayCassetteBackend`, `JevHttpBackend`. | 100% unit test coverage with recorded cassettes. |
| **Phase 2** | **Permission & Failover Upgrades** | Connect `PermissionPolicy` to `builtin_agent.rs` (`run_command`). Connect model selector to `ask_choice`. | Benign commands (`git status`) auto-allow; destructive commands prompt/block. |
| **Phase 3** | **AG-UI Streaming Server** | Embedded SSE/WebSocket daemon in `term-ed` broadcasting agent state & AEP diffs. | Can stream real-time events to `curl` or browser WebSocket client. |
| **Phase 4** | **MCP Tool Provider Extraction** | Wrap `read_file`, `edit_file`, and `run_command` in standard MCP schemas. | Any standard MCP client (Claude Code, Cursor) can consume term-ed tools. |
| **Phase 5** | **Visual Companion Editor** | React + React Flow + CopilotKit editor displaying the interactive routing graph. | Drag-and-drop node editing updates `term-ed` routing graph JSON in real time. |

---

## 9. Conclusion

By adopting **MCP for tools**, **A2A for agent delegation**, **AG-UI for UI event streaming**, **A2UI / AEP for declarative interface and editing mutations**, and **Jev for typed decision governance**, `Ziggyware/term-ed` transforms from a standalone terminal agent into an extensible, enterprise-grade multi-agent workstation. It retains its pure-Rust, zero-configuration, offline-first terminal DNA while seamlessly projecting its execution into visual graph environments.
