//! AI coding agents over the Agent Client Protocol (ACP).
//!
//! termed is an ACP *client*: it launches an agent (OpenCode, Gemini CLI,
//! Claude Code, Codex, ...) as a subprocess and talks JSON-RPC 2.0 over its
//! stdin/stdout. Out of the box it uses OpenCode, whose free models need no
//! account or API key, so the panel works with zero configuration.
//!
//! The protocol runs on a reader thread that forwards parsed messages over a
//! channel; everything else happens on the UI thread in `poll()`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// How to launch an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSpec {
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
    /// Usable without an account or payment.
    pub free: bool,
    /// termed's own in-process agent (no install needed).
    pub builtin: bool,
}

/// termed's built-in agent: free models, nothing to install. Works on
/// Termux/Android, where OpenCode can't be installed.
pub fn builtin_spec() -> AgentSpec {
    AgentSpec {
        name: "termed AI".into(),
        program: String::new(),
        args: Vec::new(),
        free: true,
        builtin: true,
    }
}

/// Known ACP agents, in order of preference. OpenCode comes first because
/// its bundled free models work without signing in.
const KNOWN: &[(&str, &str, &[&str], bool)] = &[
    ("OpenCode", "opencode", &["acp"], true),
    ("Gemini CLI", "gemini", &["--acp"], false),
    ("Claude Code", "claude-agent-acp", &[], false),
    ("Claude Code", "claude-code-acp", &[], false),
    ("Codex", "codex-acp", &[], false),
];

/// Look a program up on PATH (plus npm's global bin, where installs land).
pub fn which(program: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".opencode/bin"));
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".npm-global/bin"));
    }
    dirs.into_iter()
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

fn base_name(p: &str) -> String {
    Path::new(p)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string())
}

/// Agents available on this machine. `TERMED_AGENT="cmd args"` overrides.
pub fn detect_agents() -> Vec<AgentSpec> {
    let mut out = Vec::new();
    let chosen = std::env::var("TERMED_AGENT").ok();
    if chosen.as_deref() == Some("builtin") {
        out.push(builtin_spec());
    } else if let Some(cmd) = chosen {
        let mut parts = cmd.split_whitespace().map(String::from);
        if let Some(program) = parts.next() {
            let args: Vec<String> = parts.collect();
            // `python3 my_agent.py` is better named after the script.
            let shown = match args.first() {
                Some(a)
                    if ["python", "python3", "node", "npx", "bun", "deno", "uvx"]
                        .contains(&base_name(&program).as_str()) =>
                {
                    base_name(a)
                }
                _ => base_name(&program),
            };
            out.push(AgentSpec {
                name: shown,
                program,
                args,
                free: false,
                builtin: false,
            });
        }
    }
    // On Android (Termux) the external agents' npm packages don't install,
    // so the built-in agent comes first there.
    if cfg!(target_os = "android") && out.is_empty() {
        out.push(builtin_spec());
    }
    for (name, program, args, free) in KNOWN {
        if let Some(path) = which(program) {
            if out.iter().any(|s: &AgentSpec| s.name == *name) {
                continue;
            }
            out.push(AgentSpec {
                name: name.to_string(),
                program: path.to_string_lossy().into_owned(),
                args: args.iter().map(|s| s.to_string()).collect(),
                free: *free,
                builtin: false,
            });
        }
    }
    if !out.iter().any(|s| s.builtin) {
        out.push(builtin_spec());
    }
    out
}

// ------------------------------------------------------------ conversation

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    Pending,
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug)]
pub struct DiffView {
    pub path: PathBuf,
    pub old: Option<String>,
    pub new: String,
}

#[derive(Clone, Debug)]
pub struct ToolCard {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub status: ToolStatus,
    pub locations: Vec<(PathBuf, Option<u32>)>,
    pub diffs: Vec<DiffView>,
    pub text: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PermissionCard {
    pub rpc_id: Value,
    pub title: String,
    /// (option id, label, kind) — kind is allow_once / allow_always / reject_once / reject_always.
    pub options: Vec<(String, String, String)>,
    pub chosen: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanStatus {
    Pending,
    Running,
    Done,
}

#[derive(Clone, Debug)]
pub enum Entry {
    User(String),
    Agent(String),
    Thought(String),
    Tool(ToolCard),
    Plan(Vec<(String, PlanStatus)>),
    Permission(PermissionCard),
    Notice { text: String, error: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Not launched yet (launches on the first prompt).
    Idle,
    NoAgent,
    Installing,
    Starting,
    Ready,
    Working,
    Failed(String),
}

/// (old, new) text snippets of one edit.
pub type Snippet = (Option<String>, String);

/// Something the editor has to do on the agent's behalf.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// The agent changed a file on disk. `diffs` hold (old, new) snippets.
    FileChanged {
        path: PathBuf,
        diffs: Vec<(Option<String>, String)>,
    },
    /// fs/read_text_file: reply with `respond_read`.
    ReadFile {
        rpc_id: Value,
        path: PathBuf,
        line: Option<usize>,
        limit: Option<usize>,
    },
    /// fs/write_text_file: reply with `respond(rpc_id, Value::Null)`.
    WriteFile {
        rpc_id: Value,
        path: PathBuf,
        content: String,
    },
    TurnEnded,
}

enum Inbound {
    Message(Value),
    Exited,
    InstallLine(String),
    InstallDone(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Initialize,
    NewSession,
    Prompt,
    SetModel,
    Other,
}

pub struct Agent {
    pub available: Vec<AgentSpec>,
    pub spec: Option<AgentSpec>,
    pub state: State,
    pub entries: Vec<Entry>,
    /// (value, label) of models the agent offers, and the current one.
    pub models: Vec<(String, String)>,
    pub model: Option<String>,
    model_config_id: String,
    pub agent_title: String,
    /// Tokens used / context size / cost of the session.
    pub usage: Option<(u64, u64, f64)>,
    pub embedded_context: bool,
    pub auth_hint: Option<String>,
    pub turn_started: Option<Instant>,
    pub install_log: Vec<String>,
    // UI state for the input box and transcript.
    pub input: String,
    pub cursor: usize,
    /// Rows scrolled up from the bottom of the transcript (0 = follow).
    pub scroll: usize,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    builtin_tx: Option<Sender<Value>>,
    tx: Sender<Inbound>,
    rx: Receiver<Inbound>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    session_id: Option<String>,
    queued: Option<Vec<Value>>,
    cwd: PathBuf,
    reported: HashMap<String, ToolStatus>,
    /// Updates received in the current turn and when the last one arrived.
    turn_updates: usize,
    last_activity: Instant,
    /// The current turn's prompt, kept so a stalled turn can be retried.
    last_prompt: Option<Vec<Value>>,
    /// Free models that went silent this session (rate-limited).
    busy_models: HashSet<String>,
    retry_after_cancel: bool,
    stall_noted: bool,
    /// How long a turn may stay completely silent before we act.
    pub stall_after: Duration,
    /// Real-time AG-UI streaming hub.
    pub agui: crate::agui::AGUIHub,
}

impl Agent {
    pub fn new(cwd: PathBuf) -> Self {
        let available = detect_agents();
        let (tx, rx) = channel();
        Self {
            spec: available.first().cloned(),
            state: if available.is_empty() {
                State::NoAgent
            } else {
                State::Idle
            },
            available,
            entries: Vec::new(),
            models: Vec::new(),
            model: None,
            model_config_id: "model".into(),
            agent_title: String::new(),
            usage: None,
            embedded_context: false,
            auth_hint: None,
            turn_started: None,
            install_log: Vec::new(),
            input: String::new(),
            cursor: 0,
            scroll: 0,
            child: None,
            stdin: None,
            builtin_tx: None,
            tx,
            rx,
            stderr: Arc::new(Mutex::new(VecDeque::new())),
            next_id: 0,
            pending: HashMap::new(),
            session_id: None,
            queued: None,
            cwd,
            reported: HashMap::new(),
            turn_updates: 0,
            last_activity: Instant::now(),
            last_prompt: None,
            busy_models: HashSet::new(),
            retry_after_cancel: false,
            stall_noted: false,
            stall_after: Duration::from_secs(25),
            agui: crate::agui::AGUIHub::new(),
        }
    }

    pub fn name(&self) -> String {
        if !self.agent_title.is_empty() {
            return self.agent_title.clone();
        }
        self.spec
            .as_ref()
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "Agent".into())
    }

    pub fn is_working(&self) -> bool {
        matches!(
            self.state,
            State::Working | State::Starting | State::Installing
        )
    }

    pub fn running(&self) -> bool {
        self.child.is_some() || self.builtin_tx.is_some()
    }

    pub fn model_label(&self) -> Option<String> {
        let m = self.model.as_ref()?;
        Some(
            self.models
                .iter()
                .find(|(v, _)| v == m)
                .map(|(_, l)| l.clone())
                .unwrap_or_else(|| m.clone()),
        )
    }

    pub fn pending_permission(&self) -> Option<usize> {
        self.entries
            .iter()
            .rposition(|e| matches!(e, Entry::Permission(p) if p.chosen.is_none()))
    }

    // ------------------------------------------------------------ lifecycle

    /// Launch the agent process and begin the handshake.
    pub fn start(&mut self) {
        if self.running() {
            return;
        }
        let Some(spec) = self.spec.clone() else {
            self.state = State::NoAgent;
            return;
        };
        if spec.builtin {
            let tx = self.tx.clone();
            self.builtin_tx = Some(crate::builtin_agent::spawn(self.cwd.clone(), move |v| {
                let _ = tx.send(Inbound::Message(v));
            }));
            self.state = State::Starting;
            self.handshake();
            return;
        }
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.state = State::Failed(format!("Could not start {}: {e}", spec.name));
                return;
            }
        };
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        self.stdin = child.stdin.take();
        self.child = Some(child);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(v) = serde_json::from_str::<Value>(&line)
                    && tx.send(Inbound::Message(v)).is_err()
                {
                    return;
                }
            }
            let _ = tx.send(Inbound::Exited);
        });
        let tail = self.stderr.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut t = tail.lock().unwrap();
                t.push_back(line);
                while t.len() > 40 {
                    t.pop_front();
                }
            }
        });
        self.state = State::Starting;
        self.handshake();
    }

    fn handshake(&mut self) {
        self.request(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": { "readTextFile": true, "writeTextFile": true },
                    "terminal": false
                },
                "clientInfo": { "name": "termed", "version": env!("CARGO_PKG_VERSION") }
            }),
            Pending::Initialize,
        );
    }

    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.stdin = None;
        self.builtin_tx = None;
        self.session_id = None;
        self.pending.clear();
        if !matches!(self.state, State::NoAgent | State::Installing) {
            self.state = State::Idle;
        }
    }

    /// Switch to another detected agent (restarts the session).
    pub fn use_agent(&mut self, idx: usize) {
        if let Some(spec) = self.available.get(idx).cloned() {
            self.stop();
            self.spec = Some(spec);
            self.models.clear();
            self.model = None;
            self.agent_title.clear();
            self.entries.clear();
            self.state = State::Idle;
            self.start();
        }
    }

    /// Fresh conversation in the same agent.
    pub fn new_chat(&mut self) {
        self.entries.clear();
        self.usage = None;
        self.scroll = 0;
        self.reported.clear();
        if self.running() && self.session_id.is_some() {
            self.session_id = None;
            self.state = State::Starting;
            self.new_session();
        }
    }

    /// Install OpenCode through npm (the free default agent).
    pub fn install(&mut self) {
        if self.state == State::Installing {
            return;
        }
        if cfg!(target_os = "android") {
            self.install_log = vec![
                "OpenCode has no Android build on npm.".into(),
                "Use the built-in termed AI agent instead (Agent: Choose Agent): it is free and needs no install.".into(),
            ];
            return;
        }
        let Some(npm) = which("npm") else {
            self.install_log = vec![
                "npm was not found. Install Node.js first, then tap Install again:".into(),
                "  Termux:  pkg install nodejs".into(),
                "  Debian/Ubuntu:  sudo apt install nodejs npm".into(),
                "  macOS:  brew install node".into(),
                "Or install OpenCode directly: https://opencode.ai".into(),
            ];
            return;
        };
        self.state = State::Installing;
        self.install_log = vec!["$ npm install -g opencode-ai".into()];
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let child = Command::new(npm)
                .args(["install", "-g", "opencode-ai"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
            let ok = match child {
                Ok(mut c) => {
                    if let Some(out) = c.stderr.take() {
                        for l in BufReader::new(out).lines().map_while(Result::ok) {
                            let _ = tx.send(Inbound::InstallLine(l));
                        }
                    }
                    c.wait().is_ok_and(|s| s.success())
                }
                Err(e) => {
                    let _ = tx.send(Inbound::InstallLine(e.to_string()));
                    false
                }
            };
            let _ = tx.send(Inbound::InstallDone(ok));
        });
    }

    // ------------------------------------------------------------ prompting

    /// Send a prompt. `context` holds extra ACP content blocks (open file,
    /// selection). Launches the agent on first use.
    pub fn prompt(&mut self, text: &str, context: Vec<Value>) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.entries.push(Entry::User(text.to_string()));
        self.scroll = 0;
        let mut blocks = vec![json!({ "type": "text", "text": text })];
        blocks.extend(context);
        self.turn_started = Some(Instant::now());
        if self.session_id.is_some() && self.state == State::Ready {
            self.send_prompt(blocks);
        } else {
            self.queued = Some(blocks);
            if !self.running() {
                if matches!(self.state, State::Failed(_)) {
                    self.state = State::Idle;
                }
                self.start();
            }
        }
    }

    fn send_prompt(&mut self, blocks: Vec<Value>) {
        let sid = self.session_id.clone().unwrap_or_default();
        self.state = State::Working;
        self.turn_updates = 0;
        self.last_activity = Instant::now();
        self.stall_noted = false;
        self.last_prompt = Some(blocks.clone());
        self.request(
            "session/prompt",
            json!({ "sessionId": sid, "prompt": blocks }),
            Pending::Prompt,
        );
    }

    /// Stop the current turn (and refuse a pending permission request).
    pub fn cancel(&mut self) {
        if let Some(i) = self.pending_permission() {
            self.choose(i, None);
        }
        if let Some(sid) = self.session_id.clone() {
            self.notify("session/cancel", json!({ "sessionId": sid }));
        }
        if self.queued.take().is_some() && self.state != State::Working {
            self.entries.push(Entry::Notice {
                text: "Cancelled.".into(),
                error: false,
            });
        }
    }

    pub fn set_model(&mut self, value: &str) {
        let Some(sid) = self.session_id.clone() else {
            return;
        };
        self.model = Some(value.to_string());
        let id = self.model_config_id.clone();
        self.request(
            "session/set_config_option",
            json!({ "sessionId": sid, "configId": id, "value": value }),
            Pending::SetModel,
        );
    }

    /// Answer a permission card. `None` cancels.
    pub fn choose(&mut self, entry: usize, option: Option<&str>) {
        let Some(Entry::Permission(p)) = self.entries.get_mut(entry) else {
            return;
        };
        if p.chosen.is_some() {
            return;
        }
        let outcome = match option {
            Some(id) => json!({ "outcome": "selected", "optionId": id }),
            None => json!({ "outcome": "cancelled" }),
        };
        p.chosen = Some(
            option
                .and_then(|id| p.options.iter().find(|o| o.0 == id).map(|o| o.1.clone()))
                .unwrap_or_else(|| "Cancelled".into()),
        );
        let rpc = p.rpc_id.clone();
        self.respond(rpc, json!({ "outcome": outcome }));
    }

    // ------------------------------------------------------------ JSON-RPC

    fn write(&mut self, msg: Value) {
        if let Some(b) = &self.builtin_tx {
            if b.send(msg).is_err() {
                self.builtin_tx = None;
            }
            return;
        }
        if let Some(stdin) = &mut self.stdin {
            let mut line = msg.to_string();
            line.push('\n');
            if stdin
                .write_all(line.as_bytes())
                .and_then(|_| stdin.flush())
                .is_err()
            {
                self.stdin = None;
            }
        }
    }

    fn request(&mut self, method: &str, params: Value, kind: Pending) {
        self.next_id += 1;
        let id = self.next_id;
        self.pending.insert(id, kind);
        self.write(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.write(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    pub fn respond(&mut self, id: Value, result: Value) {
        self.write(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    pub fn respond_error(&mut self, id: Value, message: &str) {
        self.write(
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": message } }),
        );
    }

    fn new_session(&mut self) {
        let cwd = self.cwd.to_string_lossy().into_owned();
        self.request(
            "session/new",
            json!({ "cwd": cwd, "mcpServers": [] }),
            Pending::NewSession,
        );
    }

    /// Free models are shared and get rate-limited; OpenCode then retries
    /// silently. If a turn has produced nothing at all for `stall_after`,
    /// switch to another free model and resend, so it "just works".
    fn check_stall(&mut self) -> bool {
        if self.state != State::Working
            || self.turn_updates > 0
            || self.retry_after_cancel
            || self.last_activity.elapsed() < self.stall_after
        {
            return false;
        }
        let current = self.model.clone().unwrap_or_default();
        let next = if crate::app::is_free_model(&current) {
            self.busy_models.insert(current.clone());
            let free_candidates: Vec<(String, String)> = self
                .models
                .iter()
                .filter(|(v, _)| crate::app::is_free_model(v))
                .cloned()
                .collect();
            let selector = crate::decisions::ModelSelector::new(crate::decisions::default_engine());
            selector
                .select_fallback(&current, &free_candidates, &self.busy_models)
                .and_then(|sel| self.models.iter().find(|(v, _)| *v == sel).cloned())
                .or_else(|| {
                    self.models
                        .iter()
                        .find(|(v, _)| crate::app::is_free_model(v) && !self.busy_models.contains(v))
                        .cloned()
                })
        } else {
            None
        };
        match next {
            Some((value, label)) => {
                self.agui.emit(
                    "decision:evaluated",
                    self.session_id.as_deref().unwrap_or("agent"),
                    json!({
                        "query": "model_failover_choice",
                        "previous_model": current,
                        "selected_model": value,
                        "label": label,
                    }),
                );
                let old = self.model_label().unwrap_or(current);
                let short = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
                self.entries.push(Entry::Notice {
                    text: format!(
                        "{} is busy right now (free models get rate-limited). Switched to {} and asked again.",
                        short(&old),
                        short(&label)
                    ),
                    error: false,
                });
                self.retry_after_cancel = true;
                if let Some(sid) = self.session_id.clone() {
                    self.notify("session/cancel", json!({ "sessionId": sid }));
                }
                self.set_model(&value);
                self.last_activity = Instant::now();
                true
            }
            None if !self.stall_noted => {
                self.stall_noted = true;
                self.entries.push(Entry::Notice {
                    text: "No reply yet. The model may be busy: tap the model name above to pick another, or ■ to stop.".into(),
                    error: false,
                });
                true
            }
            None => false,
        }
    }

    /// Process everything the agent sent since the last call. Returns the
    /// editor-side actions to perform and whether anything changed.
    pub fn poll(&mut self) -> (Vec<AgentEvent>, bool) {
        let mut events = Vec::new();
        let mut changed = self.check_stall();
        while let Ok(msg) = self.rx.try_recv() {
            changed = true;
            match msg {
                Inbound::Message(v) => self.handle(v, &mut events),
                Inbound::Exited => self.exited(),
                Inbound::InstallLine(l) => {
                    self.install_log.push(l);
                    if self.install_log.len() > 12 {
                        self.install_log.remove(1);
                    }
                }
                Inbound::InstallDone(ok) => {
                    self.available = detect_agents();
                    self.spec = self.available.first().cloned();
                    if ok && self.spec.is_some() {
                        self.install_log.push("Installed. Starting…".into());
                        self.state = State::Idle;
                        self.start();
                    } else {
                        self.install_log
                            .push("Install failed — see the messages above.".into());
                        self.state = State::NoAgent;
                    }
                }
            }
        }
        (events, changed)
    }

    fn exited(&mut self) {
        let was_running = self.child.is_some();
        self.child = None;
        self.stdin = None;
        self.session_id = None;
        self.pending.clear();
        if was_running {
            let tail: Vec<String> = self
                .stderr
                .lock()
                .unwrap()
                .iter()
                .rev()
                .take(6)
                .rev()
                .cloned()
                .collect();
            let detail = tail.join("\n");
            self.state = State::Failed(if detail.trim().is_empty() {
                format!("{} stopped.", self.name())
            } else {
                format!("{} stopped:\n{detail}", self.name())
            });
        }
    }

    fn handle(&mut self, v: Value, events: &mut Vec<AgentEvent>) {
        let method = v.get("method").and_then(Value::as_str).map(String::from);
        let id = v.get("id").cloned();
        match (method, id) {
            // Response to one of our requests.
            (None, Some(id)) => {
                let kind = id
                    .as_u64()
                    .and_then(|n| self.pending.remove(&n))
                    .unwrap_or(Pending::Other);
                if let Some(err) = v.get("error") {
                    self.on_error(kind, err);
                } else {
                    let result = v.get("result").cloned().unwrap_or(Value::Null);
                    self.on_result(kind, result, events);
                }
            }
            // Notification.
            (Some(m), None) => {
                if m == "session/update"
                    && let Some(u) = v.pointer("/params/update")
                {
                    self.on_update(u, events);
                }
            }
            // Request from the agent.
            (Some(m), Some(id)) => {
                let p = v.get("params").cloned().unwrap_or(Value::Null);
                match m.as_str() {
                    "session/request_permission" => {
                        let title = p
                            .pointer("/toolCall/title")
                            .and_then(Value::as_str)
                            .unwrap_or("the agent wants to run a tool")
                            .to_string();
                        let options = p
                            .get("options")
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .map(|o| {
                                        let s = |k: &str| {
                                            o.get(k)
                                                .and_then(Value::as_str)
                                                .unwrap_or("")
                                                .to_string()
                                        };
                                        (s("optionId"), s("name"), s("kind"))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        self.entries.push(Entry::Permission(PermissionCard {
                            rpc_id: id,
                            title,
                            options,
                            chosen: None,
                        }));
                        self.scroll = 0;
                    }
                    "fs/read_text_file" => {
                        let path =
                            PathBuf::from(p.get("path").and_then(Value::as_str).unwrap_or(""));
                        let num = |k: &str| p.get(k).and_then(Value::as_u64).map(|n| n as usize);
                        events.push(AgentEvent::ReadFile {
                            rpc_id: id,
                            path,
                            line: num("line"),
                            limit: num("limit"),
                        });
                    }
                    "fs/write_text_file" => {
                        let path =
                            PathBuf::from(p.get("path").and_then(Value::as_str).unwrap_or(""));
                        let content = p
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        events.push(AgentEvent::WriteFile {
                            rpc_id: id,
                            path,
                            content,
                        });
                    }
                    _ => {
                        self.write(json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": { "code": -32601, "message": format!("termed does not support {m}") }
                        }));
                    }
                }
            }
            _ => {}
        }
    }

    fn on_result(&mut self, kind: Pending, r: Value, events: &mut Vec<AgentEvent>) {
        match kind {
            Pending::Initialize => {
                if let Some(name) = r
                    .pointer("/agentInfo/title")
                    .or(r.pointer("/agentInfo/name"))
                    .and_then(Value::as_str)
                {
                    self.agent_title = name.to_string();
                }
                self.embedded_context = r
                    .pointer("/agentCapabilities/promptCapabilities/embeddedContext")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                self.auth_hint = r
                    .get("authMethods")
                    .and_then(Value::as_array)
                    .and_then(|a| a.first())
                    .and_then(|m| m.get("description").or(m.get("name")))
                    .and_then(Value::as_str)
                    .map(String::from);
                self.new_session();
            }
            Pending::NewSession => {
                self.session_id = r.get("sessionId").and_then(Value::as_str).map(String::from);
                self.read_models(&r);
                self.state = State::Ready;
                if let Some(blocks) = self.queued.take() {
                    self.send_prompt(blocks);
                }
            }
            Pending::Prompt => {
                self.state = State::Ready;
                let reason = r
                    .get("stopReason")
                    .and_then(Value::as_str)
                    .unwrap_or("end_turn");
                if self.retry_after_cancel {
                    // A stalled free model was swapped out: resend the same prompt.
                    self.retry_after_cancel = false;
                    if let Some(blocks) = self.last_prompt.clone() {
                        self.send_prompt(blocks);
                        return;
                    }
                }
                let note = match reason {
                    "end_turn" => None,
                    "cancelled" => Some("Stopped."),
                    "max_tokens" => Some("Stopped: the reply hit the token limit."),
                    "max_turn_requests" => Some("Stopped: too many steps in one turn."),
                    "refusal" => Some("The agent declined this request."),
                    _ => None,
                };
                if let Some(n) = note {
                    self.entries.push(Entry::Notice {
                        text: n.into(),
                        error: false,
                    });
                }
                // Anything still marked running is done now.
                for e in &mut self.entries {
                    if let Entry::Tool(t) = e
                        && matches!(t.status, ToolStatus::Pending | ToolStatus::Running)
                    {
                        t.status = if reason == "cancelled" {
                            ToolStatus::Failed
                        } else {
                            ToolStatus::Done
                        };
                    }
                }
                self.turn_started = None;
                events.push(AgentEvent::TurnEnded);
            }
            Pending::SetModel => self.read_models(&r),
            Pending::Other => {}
        }
    }

    fn on_error(&mut self, kind: Pending, err: &Value) {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("error")
            .to_string();
        let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
        match kind {
            Pending::Initialize | Pending::NewSession => {
                let mut text = format!("Could not start a session: {msg}");
                if code == -32000
                    && let Some(h) = &self.auth_hint
                {
                    text = format!("{} needs you to sign in: {h}", self.name());
                }
                self.state = State::Failed(text);
                self.queued = None;
            }
            Pending::Prompt => {
                self.state = State::Ready;
                self.turn_started = None;
                self.entries.push(Entry::Notice {
                    text: msg,
                    error: true,
                });
            }
            Pending::SetModel | Pending::Other => {
                self.entries.push(Entry::Notice {
                    text: msg,
                    error: true,
                });
            }
        }
    }

    /// Pick up the model selector from session/new (configOptions) or the
    /// older `models` field.
    fn read_models(&mut self, r: &Value) {
        if let Some(opts) = r.get("configOptions").and_then(Value::as_array) {
            for o in opts {
                let is_model = o.get("category").and_then(Value::as_str) == Some("model")
                    || o.get("id").and_then(Value::as_str) == Some("model");
                if !is_model {
                    continue;
                }
                self.model_config_id = o
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("model")
                    .to_string();
                self.model = o
                    .get("currentValue")
                    .and_then(Value::as_str)
                    .map(String::from);
                self.models = o
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|m| {
                                let v = m.get("value").and_then(Value::as_str)?;
                                let n = m.get("name").and_then(Value::as_str).unwrap_or(v);
                                Some((v.to_string(), n.to_string()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            }
        } else if let Some(m) = r.get("models") {
            self.model = m
                .get("currentModelId")
                .and_then(Value::as_str)
                .map(String::from);
            self.models = m
                .get("availableModels")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|m| {
                            let v = m.get("modelId").and_then(Value::as_str)?;
                            let n = m.get("name").and_then(Value::as_str).unwrap_or(v);
                            Some((v.to_string(), n.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
    }

    fn on_update(&mut self, u: &Value, events: &mut Vec<AgentEvent>) {
        let kind = u.get("sessionUpdate").and_then(Value::as_str).unwrap_or("");
        if !matches!(
            kind,
            "available_commands_update"
                | "usage_update"
                | "config_option_update"
                | "session_info_update"
        ) {
            self.turn_updates += 1;
            self.last_activity = Instant::now();
        }
        let text_of = |u: &Value| -> String {
            match u.pointer("/content/type").and_then(Value::as_str) {
                Some("text") => u
                    .pointer("/content/text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                Some("image") => "[image]".into(),
                Some("resource_link") => {
                    format!(
                        "[{}]",
                        u.pointer("/content/name")
                            .and_then(Value::as_str)
                            .unwrap_or("link")
                    )
                }
                _ => String::new(),
            }
        };
        match kind {
            "agent_message_chunk" => {
                let t = text_of(u);
                match self.entries.last_mut() {
                    Some(Entry::Agent(s)) => s.push_str(&t),
                    _ => self.entries.push(Entry::Agent(t)),
                }
            }
            "agent_thought_chunk" => {
                let t = text_of(u);
                match self.entries.last_mut() {
                    Some(Entry::Thought(s)) => s.push_str(&t),
                    _ => self.entries.push(Entry::Thought(t)),
                }
            }
            "tool_call" | "tool_call_update" => {
                let id = u
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let idx = self
                    .entries
                    .iter()
                    .position(|e| matches!(e, Entry::Tool(t) if t.id == id));
                let idx = idx.unwrap_or_else(|| {
                    self.entries.push(Entry::Tool(ToolCard {
                        id: id.clone(),
                        title: String::new(),
                        kind: "other".into(),
                        status: ToolStatus::Pending,
                        locations: Vec::new(),
                        diffs: Vec::new(),
                        text: None,
                    }));
                    self.entries.len() - 1
                });
                let Entry::Tool(t) = &mut self.entries[idx] else {
                    return;
                };
                if let Some(s) = u.get("title").and_then(Value::as_str) {
                    t.title = s.to_string();
                }
                if let Some(s) = u.get("kind").and_then(Value::as_str) {
                    t.kind = s.to_string();
                }
                if let Some(s) = u.get("status").and_then(Value::as_str) {
                    t.status = match s {
                        "in_progress" => ToolStatus::Running,
                        "completed" => ToolStatus::Done,
                        "failed" => ToolStatus::Failed,
                        _ => ToolStatus::Pending,
                    };
                }
                if let Some(locs) = u.get("locations").and_then(Value::as_array)
                    && !locs.is_empty()
                {
                    t.locations = locs
                        .iter()
                        .filter_map(|l| {
                            let p = l.get("path").and_then(Value::as_str)?;
                            Some((
                                PathBuf::from(p),
                                l.get("line").and_then(Value::as_u64).map(|n| n as u32),
                            ))
                        })
                        .collect();
                }
                if let Some(content) = u.get("content").and_then(Value::as_array) {
                    t.diffs.clear();
                    t.text = None;
                    for c in content {
                        match c.get("type").and_then(Value::as_str) {
                            Some("diff") => t.diffs.push(DiffView {
                                path: PathBuf::from(
                                    c.get("path").and_then(Value::as_str).unwrap_or(""),
                                ),
                                old: c.get("oldText").and_then(Value::as_str).map(String::from),
                                new: c
                                    .get("newText")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string(),
                            }),
                            Some("content") => {
                                if let Some(s) = c.pointer("/content/text").and_then(Value::as_str)
                                {
                                    let cur = t.text.get_or_insert_with(String::new);
                                    if !cur.is_empty() {
                                        cur.push('\n');
                                    }
                                    cur.push_str(s);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                // Tell the editor once per finished edit so it can reload buffers.
                let edits =
                    matches!(t.kind.as_str(), "edit" | "delete" | "move") || !t.diffs.is_empty();
                if t.status == ToolStatus::Done
                    && edits
                    && self.reported.get(&t.id) != Some(&ToolStatus::Done)
                {
                    self.reported.insert(t.id.clone(), ToolStatus::Done);
                    let mut by_path: Vec<(PathBuf, Vec<Snippet>)> = Vec::new();
                    for d in &t.diffs {
                        match by_path.iter_mut().find(|(p, _)| *p == d.path) {
                            Some((_, v)) => v.push((d.old.clone(), d.new.clone())),
                            None => {
                                by_path.push((d.path.clone(), vec![(d.old.clone(), d.new.clone())]))
                            }
                        }
                    }
                    for (p, _) in &t.locations {
                        if !by_path.iter().any(|(q, _)| q == p) {
                            by_path.push((p.clone(), Vec::new()));
                        }
                    }
                    for (path, diffs) in by_path {
                        events.push(AgentEvent::FileChanged { path, diffs });
                    }
                }
            }
            "plan" => {
                let items = u
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|e| {
                                let c = e
                                    .get("content")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string();
                                let s = match e.get("status").and_then(Value::as_str) {
                                    Some("completed") => PlanStatus::Done,
                                    Some("in_progress") => PlanStatus::Running,
                                    _ => PlanStatus::Pending,
                                };
                                (c, s)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                match self
                    .entries
                    .iter_mut()
                    .rev()
                    .find(|e| matches!(e, Entry::Plan(_)))
                {
                    Some(Entry::Plan(p)) => *p = items,
                    _ => self.entries.push(Entry::Plan(items)),
                }
            }
            "usage_update" => {
                let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                let cost = u
                    .pointer("/cost/amount")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                self.usage = Some((n("used"), n("size"), cost));
            }
            "notice" => {
                let title = u
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let error = u.get("severity").and_then(Value::as_str) == Some("error");
                self.entries.push(Entry::Notice { text: title, error });
            }
            "config_option_update" => {
                self.read_models(&json!({ "configOptions": u.get("configOptions") }))
            }
            _ => {}
        }
    }

    /// Reply to fs/read_text_file with the requested slice of `text`.
    pub fn respond_read(
        &mut self,
        id: Value,
        text: &str,
        line: Option<usize>,
        limit: Option<usize>,
    ) {
        let content = slice_lines(text, line, limit);
        self.respond(id, json!({ "content": content }));
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Lines `line..line+limit` (1-based) of `text`, as fs/read_text_file asks.
pub fn slice_lines(text: &str, line: Option<usize>, limit: Option<usize>) -> String {
    if line.is_none() && limit.is_none() {
        return text.to_string();
    }
    let skip = line.unwrap_or(1).saturating_sub(1);
    text.split_inclusive('\n')
        .skip(skip)
        .take(limit.unwrap_or(usize::MAX))
        .collect()
}

/// `file://` URI for a path (ACP resource links).
pub fn file_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> Agent {
        let mut a = Agent::new(PathBuf::from("/tmp"));
        a.session_id = Some("s1".into());
        a.state = State::Working;
        a
    }

    #[test]
    fn chunks_merge_and_tool_edits_are_reported_once() {
        let mut a = agent();
        let mut ev = Vec::new();
        for t in ["Let me ", "look."] {
            a.on_update(
                &json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":t}}),
                &mut ev,
            );
        }
        assert!(matches!(&a.entries[0], Entry::Agent(s) if s == "Let me look."));
        a.on_update(&json!({"sessionUpdate":"tool_call","toolCallId":"t1","title":"edit","kind":"edit","status":"pending"}), &mut ev);
        let done = json!({"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed",
            "content":[{"type":"diff","path":"/p/a.py","oldText":"a - b","newText":"a + b"}]});
        a.on_update(&done, &mut ev);
        a.on_update(&done, &mut ev);
        assert_eq!(ev.len(), 1);
        match &ev[0] {
            AgentEvent::FileChanged { path, diffs } => {
                assert_eq!(path, &PathBuf::from("/p/a.py"));
                assert_eq!(diffs[0], (Some("a - b".to_string()), "a + b".to_string()));
            }
            e => panic!("unexpected {e:?}"),
        }
    }

    #[test]
    fn permission_requests_round_trip() {
        let mut a = agent();
        let mut ev = Vec::new();
        a.handle(
            json!({"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{"sessionId":"s1",
                "toolCall":{"toolCallId":"t","title":"rm -rf build"},
                "options":[{"optionId":"ok","name":"Allow once","kind":"allow_once"},{"optionId":"no","name":"Reject","kind":"reject_once"}]}}),
            &mut ev,
        );
        let i = a.pending_permission().unwrap();
        a.choose(i, Some("ok"));
        assert!(a.pending_permission().is_none());
        match &a.entries[i] {
            Entry::Permission(p) => assert_eq!(p.chosen.as_deref(), Some("Allow once")),
            _ => panic!(),
        }
    }

    #[test]
    fn models_come_from_config_options() {
        let mut a = agent();
        a.read_models(&json!({"configOptions":[{"id":"model","category":"model","type":"select",
            "currentValue":"opencode/big-pickle","options":[{"value":"opencode/big-pickle","name":"Big Pickle"}]}]}));
        assert_eq!(a.model_label().as_deref(), Some("Big Pickle"));
    }

    #[test]
    fn read_slices_lines() {
        assert_eq!(slice_lines("a\nb\nc\n", Some(2), Some(1)), "b\n");
        assert_eq!(slice_lines("a\nb\nc", Some(2), None), "b\nc");
        assert_eq!(slice_lines("a\nb", None, None), "a\nb");
    }
}
