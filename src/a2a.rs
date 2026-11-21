//! Agent-to-agent (A2A) communication.
//!
//! termed already delegates to sub-agents in-process (see `automation` and
//! `supervision`). A2A adds the parts that make delegation *between programs*
//! possible without a rewrite:
//!
//! * an **agent card** (`/.well-known/agent.json`) describing what an agent can
//!   do, so a caller can discover it before sending work;
//! * **tasks** with a lifecycle (`submitted` → `working` → terminal) that can be
//!   listed and inspected, not just fire-and-forget messages;
//! * **capability negotiation**: a caller states what it needs and gets a clear
//!   refusal when the agent cannot do it.
//!
//! The wire format is JSON, and every structure is `Serialize`/`Deserialize`, so
//! the same types serve the local TUI, the AG-UI bridge and an HTTP peer. This
//! module holds the protocol and its state machine only — no I/O — so the rules
//! are testable and the transport can change without touching them.

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Terminal states never transition again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Submitted,
    Working,
    /// Waiting on the caller for input (approval, a missing detail).
    InputRequired,
    Completed,
    Failed,
    Cancelled,
}

impl TaskState {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Working => "working",
            Self::InputRequired => "input-required",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// What an agent advertises about itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCard {
    pub name: String,
    pub description: String,
    pub version: String,
    /// Stable identifier for this agent in a mesh.
    pub id: String,
    /// Capability names, e.g. `code.edit`, `shell.run`, `agent.delegate`.
    pub capabilities: Vec<String>,
    /// Skills, in the A2A sense: named jobs the agent can take on.
    pub skills: Vec<Skill>,
    /// True when the agent only reads and will not mutate the workspace.
    pub read_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
}

impl AgentCard {
    /// The built-in agent's card, derived from the live configuration so the
    /// advertised capabilities cannot drift from what the agent will do.
    pub fn for_builtin(capabilities: &[String], read_only: bool) -> Self {
        Self {
            name: "termed built-in agent".into(),
            description:
                "Read, search, edit and create files, run approved commands, and delegate to sub-agents."
                    .into(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            id: "termed-builtin".into(),
            capabilities: capabilities.to_vec(),
            skills: vec![
                Skill {
                    id: "code.edit".into(),
                    name: "Edit code".into(),
                    description: "Apply precise edits to files in the project.".into(),
                },
                Skill {
                    id: "code.review".into(),
                    name: "Review code".into(),
                    description: "Inspect a diff or file and report problems.".into(),
                },
                Skill {
                    id: "code.explain".into(),
                    name: "Explain code".into(),
                    description: "Describe what a region or symbol does.".into(),
                },
            ],
            read_only,
        }
    }

    pub fn has(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }

    /// True when every capability the caller needs is advertised. An empty
    /// request always matches: state a need, or accept whatever is offered.
    pub fn satisfies(&self, required: &[String]) -> bool {
        required.iter().all(|r| self.has(r))
    }

    /// The missing capabilities, so a refusal can name them.
    pub fn missing(&self, required: &[String]) -> Vec<String> {
        required
            .iter()
            .filter(|r| !self.has(r))
            .cloned()
            .collect()
    }

    pub fn to_json(&self) -> Value {
        json!(self)
    }
}

/// A unit of work handed to an agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    /// The agent that should do the work, when known.
    pub agent: String,
    pub state: TaskState,
    pub objective: String,
    /// Capabilities the caller requires of the agent.
    pub required: Vec<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
    /// Progress lines, newest last. Bounded so a long task cannot grow forever.
    pub updates: Vec<String>,
    /// The final answer, or the failure reason.
    pub result: Option<String>,
    /// Parent task when this was delegated by another task.
    pub parent: Option<String>,
}

impl Task {
    pub fn new(id: impl Into<String>, agent: impl Into<String>, objective: impl Into<String>) -> Self {
        let now = now_ms();
        Self {
            id: id.into(),
            agent: agent.into(),
            state: TaskState::Submitted,
            objective: objective.into(),
            required: Vec::new(),
            created_ms: now,
            updated_ms: now,
            updates: Vec::new(),
            result: None,
            parent: None,
        }
    }

    pub fn requiring(mut self, capabilities: &[String]) -> Self {
        self.required = capabilities.to_vec();
        self
    }

    pub fn child_of(mut self, parent: impl Into<String>) -> Self {
        self.parent = Some(parent.into());
        self
    }

    /// Move the task forward. Terminal states are final: a late `working`
    /// report from a cancelled task must not resurrect it.
    pub fn transition(&mut self, next: TaskState) -> Result<(), Refusal> {
        if self.state.terminal() {
            return Err(Refusal::AlreadyFinished {
                task: self.id.clone(),
                state: self.state,
            });
        }
        if next == self.state {
            return Ok(());
        }
        self.state = next;
        self.updated_ms = now_ms();
        Ok(())
    }

    pub fn progress(&mut self, line: impl Into<String>) -> Result<(), Refusal> {
        if self.state.terminal() {
            return Err(Refusal::AlreadyFinished {
                task: self.id.clone(),
                state: self.state,
            });
        }
        if self.state == TaskState::Submitted {
            self.transition(TaskState::Working)?;
        }
        self.updates.push(line.into());
        // Keep the tail: the newest progress is what a monitor needs.
        const LIMIT: usize = 200;
        if self.updates.len() > LIMIT {
            let drop = self.updates.len() - LIMIT;
            self.updates.drain(0..drop);
        }
        self.updated_ms = now_ms();
        Ok(())
    }

    pub fn finish(mut self, result: impl Into<String>) -> Self {
        self.state = TaskState::Completed;
        self.result = Some(result.into());
        self.updated_ms = now_ms();
        self
    }

    pub fn fail(mut self, reason: impl Into<String>) -> Self {
        self.state = TaskState::Failed;
        self.result = Some(reason.into());
        self.updated_ms = now_ms();
        self
    }
}

/// Why an A2A operation was refused. Every variant carries enough detail to
/// explain the refusal to a human *and* to another agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    MissingCapabilities {
        agent: String,
        missing: Vec<String>,
    },
    AlreadyFinished {
        task: String,
        state: TaskState,
    },
    UnknownTask(String),
    Duplicate(String),
    /// Delegation would exceed the allowed depth (a cycle in the mesh).
    TooDeep {
        depth: usize,
        limit: usize,
    },
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Self::MissingCapabilities { agent, missing } => format!(
                "{agent} does not offer {}. Ask a different agent, or relax the requirement.",
                missing.join(", ")
            ),
            Self::AlreadyFinished { task, state } => {
                format!("task {task} already finished ({})", state.label())
            }
            Self::UnknownTask(id) => format!("no such task: {id}"),
            Self::Duplicate(id) => format!("task {id} already exists"),
            Self::TooDeep { depth, limit } => format!(
                "delegation {depth} levels deep would exceed the limit of {limit}"
            ),
        }
    }

    /// A JSON-RPC shaped error, so a peer receives structured detail.
    pub fn to_json(&self, id: Option<u64>) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": self.code(), "message": self.message(), "data": self.data() },
        })
    }

    fn code(&self) -> i64 {
        match self {
            Self::MissingCapabilities { .. } => -32001,
            Self::UnknownTask(_) => -32002,
            Self::AlreadyFinished { .. } => -32003,
            Self::Duplicate(_) => -32004,
            Self::TooDeep { .. } => -32005,
        }
    }

    fn data(&self) -> Value {
        match self {
            Self::MissingCapabilities { agent, missing } => {
                json!({ "agent": agent, "missing": missing })
            }
            Self::AlreadyFinished { task, state } => {
                json!({ "task": task, "state": state })
            }
            Self::UnknownTask(id) => json!({ "task": id }),
            Self::Duplicate(id) => json!({ "task": id }),
            Self::TooDeep { depth, limit } => json!({ "depth": depth, "limit": limit }),
        }
    }
}

/// The local task ledger: what this process has been asked to do, and by whom.
#[derive(Default)]
pub struct Ledger {
    /// Newest last, bounded. Terminal tasks are eventually evicted, so a
    /// long-running session cannot grow without limit.
    tasks: VecDeque<Task>,
    limit: usize,
    max_depth: usize,
}

impl Ledger {
    pub fn new(limit: usize, max_depth: usize) -> Self {
        Self {
            tasks: VecDeque::new(),
            limit: limit.max(1),
            max_depth: max_depth.max(1),
        }
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Accept a task if the agent's card covers what the caller needs and the
    /// delegation chain is not too deep.
    pub fn submit(
        &mut self,
        mut task: Task,
        card: &AgentCard,
        depth: usize,
    ) -> Result<String, Refusal> {
        if self.tasks.iter().any(|t| t.id == task.id) {
            return Err(Refusal::Duplicate(task.id));
        }
        if depth > self.max_depth {
            return Err(Refusal::TooDeep {
                depth,
                limit: self.max_depth,
            });
        }
        let missing = card.missing(&task.required);
        if !missing.is_empty() {
            return Err(Refusal::MissingCapabilities {
                agent: card.name.clone(),
                missing,
            });
        }
        if task.agent.is_empty() {
            task.agent = card.id.clone();
        }
        let id = task.id.clone();
        self.push(task);
        Ok(id)
    }

    fn push(&mut self, task: Task) {
        // Evict the oldest terminal task before dropping work in progress.
        if self.tasks.len() >= self.limit
            && let Some(i) = self.tasks.iter().position(|t| t.state.terminal())
        {
            self.tasks.remove(i);
        }
        self.tasks.push_back(task);
        while self.tasks.len() > self.limit {
            self.tasks.pop_front();
        }
    }

    pub fn get(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// Update a task in place, surfacing a refusal when it already finished.
    pub fn update<F>(&mut self, id: &str, f: F) -> Result<(), Refusal>
    where
        F: FnOnce(&mut Task) -> Result<(), Refusal>,
    {
        let task = self
            .tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| Refusal::UnknownTask(id.to_string()))?;
        f(task)
    }

    /// Children of a task, for a delegation tree view.
    pub fn children(&self, id: &str) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| t.parent.as_deref() == Some(id))
            .collect()
    }

    pub fn snapshot(&self) -> Value {
        json!({
            "tasks": self.tasks.iter().collect::<Vec<_>>(),
            "active": self
                .tasks
                .iter()
                .filter(|t| !t.state.terminal())
                .count(),
        })
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Build a task id that is unique per process without a random source.
pub fn next_id(counter: u64) -> String {
    format!("task-{}-{counter}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> AgentCard {
        AgentCard::for_builtin(&["code.edit".into(), "shell.run".into()], false)
    }

    #[test]
    fn a_card_advertises_and_checks_capabilities() {
        let c = card();
        assert!(c.has("code.edit"));
        assert!(!c.has("shell.delete"));
        assert!(c.satisfies(&[]));
        assert!(c.satisfies(&["code.edit".into()]));
        assert!(!c.satisfies(&["code.edit".into(), "shell.delete".into()]));
        assert_eq!(c.missing(&["shell.delete".into()]), vec!["shell.delete"]);
        // The advertised JSON round-trips, so a peer sees what we see.
        let back: AgentCard = serde_json::from_value(c.to_json()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn submitting_work_checks_capabilities_first() {
        let mut ledger = Ledger::new(10, 3);
        let ok = Task::new("t1", "", "edit a file").requiring(&["code.edit".into()]);
        assert_eq!(ledger.submit(ok, &card(), 0).unwrap(), "t1");
        // The agent id is filled from the card when the caller leaves it out.
        assert_eq!(ledger.get("t1").unwrap().agent, "termed-builtin");

        let bad = Task::new("t2", "", "delete a file").requiring(&["shell.delete".into()]);
        let err = ledger.submit(bad, &card(), 0).unwrap_err();
        match &err {
            Refusal::MissingCapabilities { missing, .. } => {
                assert_eq!(missing, &vec!["shell.delete".to_string()])
            }
            other => panic!("{other:?}"),
        }
        assert!(err.message().contains("shell.delete"), "{}", err.message());
        // A refusal is structured for a peer, not a bare string.
        let json = err.to_json(Some(7));
        assert_eq!(json["error"]["code"], -32001);
        assert_eq!(json["id"], 7);
    }

    #[test]
    fn duplicate_ids_are_refused() {
        let mut ledger = Ledger::new(10, 3);
        let t = Task::new("same", "", "a");
        ledger.submit(t, &card(), 0).unwrap();
        let again = Task::new("same", "", "b");
        assert_eq!(
            ledger.submit(again, &card(), 0).unwrap_err(),
            Refusal::Duplicate("same".into())
        );
        // The original objective is untouched.
        assert_eq!(ledger.get("same").unwrap().objective, "a");
    }

    #[test]
    fn delegation_depth_is_bounded() {
        let mut ledger = Ledger::new(10, 2);
        for depth in 0..=2 {
            let t = Task::new(format!("t{depth}"), "", "go");
            assert!(ledger.submit(t, &card(), depth).is_ok(), "depth {depth}");
        }
        let deep = Task::new("t3", "", "go");
        let err = ledger.submit(deep, &card(), 3).unwrap_err();
        assert_eq!(err, Refusal::TooDeep { depth: 3, limit: 2 });
        assert!(err.message().contains("3 levels"));
    }

    #[test]
    fn lifecycle_moves_forward_and_terminal_states_are_final() {
        let mut t = Task::new("t", "", "work");
        assert_eq!(t.state, TaskState::Submitted);
        t.transition(TaskState::Working).unwrap();
        t.progress("halfway").unwrap();
        t.transition(TaskState::InputRequired).unwrap();
        assert!(!t.state.terminal());
        t.transition(TaskState::Completed).unwrap();
        assert!(t.state.terminal());

        // A late report cannot resurrect a finished task.
        let err = t.transition(TaskState::Working).unwrap_err();
        assert_eq!(
            err,
            Refusal::AlreadyFinished {
                task: "t".into(),
                state: TaskState::Completed
            }
        );
        assert!(t.progress("late").is_err());
    }

    #[test]
    fn progress_from_submitted_starts_the_task() {
        let mut t = Task::new("t", "", "work");
        t.progress("starting").unwrap();
        assert_eq!(t.state, TaskState::Working);
    }

    #[test]
    fn progress_history_is_bounded_to_the_newest_lines() {
        let mut t = Task::new("t", "", "work");
        for i in 0..250 {
            t.progress(format!("step {i}")).unwrap();
        }
        assert_eq!(t.updates.len(), 200);
        assert_eq!(t.updates.last().unwrap(), "step 249");
        assert_eq!(t.updates.first().unwrap(), "step 50");
    }

    #[test]
    fn the_ledger_prefers_evicting_finished_work() {
        let mut ledger = Ledger::new(3, 3);
        ledger.submit(Task::new("a", "", "x").finish("done"), &card(), 0).unwrap();
        ledger.submit(Task::new("b", "", "x").finish("done"), &card(), 0).unwrap();
        ledger.submit(Task::new("run", "", "x"), &card(), 0).unwrap();
        assert_eq!(ledger.len(), 3);
        // Adding a fourth drops a terminal task, not the running one.
        ledger.submit(Task::new("d", "", "x"), &card(), 0).unwrap();
        assert_eq!(ledger.len(), 3);
        assert!(ledger.get("run").is_some(), "active work survives");
        assert!(ledger.get("a").is_none(), "the oldest finished task went");
    }

    #[test]
    fn children_and_snapshot_describe_the_tree() {
        let mut ledger = Ledger::new(10, 3);
        let root = Task::new("root", "", "plan");
        ledger.submit(root, &card(), 0).unwrap();
        let child = Task::new("kid", "", "code").child_of("root");
        ledger.submit(child, &card(), 1).unwrap();
        assert_eq!(ledger.children("root").len(), 1);
        let snap = ledger.snapshot();
        assert_eq!(snap["tasks"].as_array().unwrap().len(), 2);
        assert_eq!(snap["active"], 2);
    }

    #[test]
    fn updating_a_missing_task_is_an_error_not_a_panic() {
        let mut ledger = Ledger::new(4, 2);
        let err = ledger
            .update("ghost", |t| t.progress("x"))
            .unwrap_err();
        assert_eq!(err, Refusal::UnknownTask("ghost".into()));
    }

    #[test]
    fn ids_are_unique_per_counter() {
        let a = next_id(1);
        let b = next_id(2);
        assert_ne!(a, b);
        assert!(a.starts_with(&format!("task-{}", std::process::id())));
    }
}
