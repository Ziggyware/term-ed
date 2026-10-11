//! termed's built-in coding agent: no install, no account, no API key.
//!
//! It talks to the free models of Kilo Gateway (an OpenAI-compatible API
//! that allows anonymous use of its `:free` models, rate-limited per IP) and
//! runs a small tool-using agent loop: list, read, search, edit and create
//! files, and run commands (each command needs your approval).
//!
//! It is pure Rust, so it works anywhere termed runs — including Termux on
//! Android, where Node/Bun-based agents can't be installed. It speaks the same
//! Agent Client Protocol messages as an external agent, only over in-process
//! channels, so the agent panel treats it like any other ACP agent.

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::aep::AepPatch;
use crate::agui::AGUIHub;
use crate::decisions::{
    DecisionContext, DecisionEngine, ModelSelector, PermissionPolicy, PermissionVerdict,
    default_engine,
};

pub const GATEWAY: &str = "https://api.kilo.ai/api/gateway";

/// The API base (overridable for tests and self-hosted gateways).
fn gateway() -> String {
    std::env::var("TERMED_FREE_GATEWAY").unwrap_or_else(|_| GATEWAY.to_string())
}
/// Kilo's router across its free models.
pub const DEFAULT_MODEL: &str = "kilo-auto/free";
const MAX_STEPS: usize = 30;

/// Start the agent thread. `out` receives the JSON-RPC messages an external
/// agent would print; the returned sender takes the messages we'd write to it.
pub fn spawn(cwd: PathBuf, out: impl Fn(Value) + Send + Sync + 'static) -> Sender<Value> {
    let (to_agent, inbox) = channel::<Value>();
    let (work_tx, work_rx) = channel::<Value>();
    let (reply_tx, reply_rx) = channel::<Value>();
    let cancel = Arc::new(AtomicBool::new(false));
    // Dispatcher: cancellations and permission answers must get through
    // while the worker is busy streaming a reply.
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        for msg in inbox {
            let method = msg.get("method").and_then(Value::as_str);
            match method {
                Some("session/cancel") => c2.store(true, Ordering::SeqCst),
                None => {
                    let _ = reply_tx.send(msg);
                }
                Some(_) => {
                    if work_tx.send(msg).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let out: Arc<dyn Fn(Value) + Send + Sync> = Arc::new(out);
    std::thread::spawn(move || {
        let mut w = Worker::new(cwd, out, reply_rx, cancel);
        for msg in work_rx {
            w.handle(msg);
        }
    });
    to_agent
}

fn http() -> ureq::Agent {
    let mut cfg = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(20)))
        .user_agent(concat!("termed/", env!("CARGO_PKG_VERSION")));
    // Bundled Mozilla roots work everywhere (incl. Android). If the system
    // points at its own bundle (corporate/MITM proxies), trust that instead.
    if let Ok(path) = std::env::var("SSL_CERT_FILE")
        && let Ok(pem) = std::fs::read(&path)
    {
        let certs: Vec<_> = ureq::tls::parse_pem(&pem)
            .filter_map(|i| match i {
                Ok(ureq::tls::PemItem::Certificate(c)) => Some(c.to_owned()),
                _ => None,
            })
            .collect();
        if !certs.is_empty() {
            cfg = cfg.tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs(ureq::tls::RootCerts::new_with_certs(&certs))
                    .build(),
            );
        }
    }
    cfg.build().new_agent()
}

/// Free, tool-capable models from the gateway (falls back to the router).
pub fn free_models(http: &ureq::Agent) -> Vec<(String, String)> {
    let mut out = vec![(
        DEFAULT_MODEL.to_string(),
        "Auto (best free model)".to_string(),
    )];
    let Ok(resp) = http.get(format!("{}/models", gateway())).call() else {
        return out;
    };
    let Ok(text) = resp.into_body().read_to_string() else {
        return out;
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return out;
    };
    for m in v
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let id = m.get("id").and_then(Value::as_str).unwrap_or("");
        let tools = m
            .get("supported_parameters")
            .and_then(Value::as_array)
            .is_some_and(|p| p.iter().any(|x| x == "tools"));
        if id.ends_with(":free") && tools && id != DEFAULT_MODEL {
            let name = m
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_string();
            out.push((id.to_string(), name));
        }
    }
    out
}

fn tools_schema() -> Value {
    let f = |name: &str, desc: &str, props: Value, req: &[&str]| {
        json!({"type":"function","function":{"name":name,"description":desc,
            "parameters":{"type":"object","properties":props,"required":req}}})
    };
    json!([
        f(
            "list_files",
            "List files in a directory of the project (recursive, respects .gitignore).",
            json!({"path":{"type":"string","description":"Directory relative to the project root; default '.'"}}),
            &[]
        ),
        f(
            "read_file",
            "Read a text file. Optionally a line range (1-based, inclusive).",
            json!({"path":{"type":"string"},"start_line":{"type":"integer"},"end_line":{"type":"integer"}}),
            &["path"]
        ),
        f(
            "search",
            "Search file contents with a regular expression. Returns file:line: text.",
            json!({"pattern":{"type":"string"},"path":{"type":"string","description":"Directory to search; default '.'"}}),
            &["pattern"]
        ),
        f(
            "edit_file",
            "Replace old_text with new_text in a file. old_text must match exactly once; include enough surrounding lines to make it unique.",
            json!({"path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"}}),
            &["path", "old_text", "new_text"]
        ),
        f(
            "write_file",
            "Create a file or overwrite it completely.",
            json!({"path":{"type":"string"},"content":{"type":"string"}}),
            &["path", "content"]
        ),
        f(
            "run_command",
            "Run a shell command in the project root (the user must approve it). Use for builds and tests.",
            json!({"command":{"type":"string"}}),
            &["command"]
        ),
    ])
}

struct Worker {
    cwd: PathBuf,
    out: Arc<dyn Fn(Value) + Send + Sync>,
    replies: Receiver<Value>,
    cancel: Arc<AtomicBool>,
    http: ureq::Agent,
    model: String,
    models: Vec<(String, String)>,
    history: Vec<Value>,
    next_id: u64,
    always_allow_commands: bool,
    policy: PermissionPolicy,
    model_selector: ModelSelector,
    decisions: Arc<dyn DecisionEngine>,
    agui: AGUIHub,
}

enum Step {
    Cancelled,
    Failed(String, Option<u16>),
}

impl Worker {
    fn new(
        cwd: PathBuf,
        out: Arc<dyn Fn(Value) + Send + Sync>,
        replies: Receiver<Value>,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        let decisions = default_engine();
        let auto_allow_thresh = if std::env::var("TERMED_AUTO_ALLOW").map(|v| v == "0").unwrap_or(false) {
            0.0
        } else {
            0.20
        };
        let policy = PermissionPolicy::with_thresholds(decisions.clone(), auto_allow_thresh, 0.75);
        let model_selector = ModelSelector::new(decisions.clone());
        let agui = AGUIHub::new();

        Self {
            cwd,
            out,
            replies,
            cancel,
            http: http(),
            model: std::env::var("TERMED_FREE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into()),
            models: Vec::new(),
            history: Vec::new(),
            next_id: 1000,
            always_allow_commands: false,
            policy,
            model_selector,
            decisions,
            agui,
        }
    }

    fn send(&self, v: Value) {
        (self.out)(v);
    }

    fn respond(&self, id: &Value, result: Value) {
        self.send(json!({"jsonrpc":"2.0","id":id,"result":result}));
    }

    fn update(&self, u: Value) {
        self.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"builtin","update":u}}));
    }

    fn notice(&self, text: &str, error: bool) {
        self.update(json!({"sessionUpdate":"notice","severity": if error {"error"} else {"info"},"title":text}));
    }

    fn config_options(&self) -> Value {
        let opts: Vec<Value> = self
            .models
            .iter()
            .map(|(v, n)| json!({"value":v,"name":n}))
            .collect();
        json!([{"id":"model","name":"Model","category":"model","type":"select","currentValue":self.model,"options":opts}])
    }

    fn handle(&mut self, msg: Value) {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let p = msg.get("params").cloned().unwrap_or(Value::Null);
        match msg.get("method").and_then(Value::as_str).unwrap_or("") {
            "initialize" => self.respond(
                &id,
                json!({"protocolVersion":1,
                    "agentCapabilities":{"promptCapabilities":{"embeddedContext":true}},
                    "authMethods":[],
                    "agentInfo":{"name":"termed","title":"termed AI","version":env!("CARGO_PKG_VERSION")}}),
            ),
            "session/new" => {
                if let Some(c) = p.get("cwd").and_then(Value::as_str) {
                    self.cwd = PathBuf::from(c);
                }
                self.history.clear();
                if self.models.is_empty() {
                    self.models = free_models(&self.http);
                }
                if !self.models.iter().any(|(v, _)| *v == self.model) {
                    self.models.insert(0, (self.model.clone(), self.model.clone()));
                }
                self.respond(&id, json!({"sessionId":"builtin","configOptions":self.config_options()}));
            }
            "session/set_config_option" => {
                if let Some(v) = p.get("value").and_then(Value::as_str) {
                    self.model = v.to_string();
                }
                self.respond(&id, json!({"configOptions":self.config_options()}));
            }
            "session/prompt" => {
                self.cancel.store(false, Ordering::SeqCst);
                let reason = self.turn(&p);
                self.respond(&id, json!({"stopReason":reason}));
            }
            m if !id.is_null() => {
                self.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("unsupported: {m}")}}));
            }
            _ => {}
        }
    }

    fn system_prompt(&self) -> String {
        format!(
            "You are termed AI, a coding agent inside the termed code editor. The project root is {}. \
             Platform: {}. Use the tools to explore, read, edit and create files and to run commands \
             (the user approves each command). Read a file before editing it, and prefer edit_file \
             with a small, unique old_text over rewriting whole files. Paths are relative to the \
             project root. When you are done, reply briefly in Markdown: what you changed and why.",
            self.cwd.display(),
            if cfg!(target_os = "android") {
                "Android (Termux)"
            } else {
                std::env::consts::OS
            }
        )
    }

    /// One user turn: loop model → tools until the model stops calling tools.
    fn turn(&mut self, p: &Value) -> &'static str {
        let mut text = String::new();
        for b in p
            .get("prompt")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match b.get("type").and_then(Value::as_str) {
                Some("text") => {
                    text.push_str(b.get("text").and_then(Value::as_str).unwrap_or(""));
                    text.push_str("\n\n");
                }
                Some("resource_link") => {
                    let uri = b.get("uri").and_then(Value::as_str).unwrap_or("");
                    let path = uri.trim_start_matches("file://");
                    let rel = Path::new(path)
                        .strip_prefix(&self.cwd)
                        .map(|p| p.display().to_string())
                        .unwrap_or(path.to_string());
                    text.push_str(&format!("(The user has {rel} open in the editor.)\n"));
                }
                Some("resource") => {
                    let r = b.get("resource").cloned().unwrap_or(Value::Null);
                    let uri = r.get("uri").and_then(Value::as_str).unwrap_or("");
                    let body = r.get("text").and_then(Value::as_str).unwrap_or("");
                    text.push_str(&format!("Selected text ({uri}):\n```\n{body}\n```\n"));
                }
                _ => {}
            }
        }
        self.history
            .push(json!({"role":"user","content":text.trim()}));
        let mut tried_models = vec![self.model.clone()];
        for _ in 0..MAX_STEPS {
            if self.cancel.load(Ordering::SeqCst) {
                return "cancelled";
            }
            match self.step() {
                Ok(true) => continue,
                Ok(false) => return "end_turn",
                Err(Step::Cancelled) => return "cancelled",
                Err(Step::Failed(msg, status)) => {
                    // Busy or failing free model: move on to another free model.
                    let busy = matches!(status, Some(429) | Some(500..=599)) || status.is_none();
                    let busy_set: HashSet<String> = tried_models.iter().cloned().collect();
                    let next = self
                        .model_selector
                        .select_fallback(&self.model, &self.models, &busy_set)
                        .or_else(|| {
                            self.models
                                .iter()
                                .map(|(v, _)| v.clone())
                                .find(|v| !tried_models.contains(v))
                        });
                    match next {
                        Some(n) if busy && tried_models.len() < 4 => {
                            self.agui.emit(
                                "decision:evaluated",
                                "builtin",
                                json!({
                                    "query": "model_failover_choice",
                                    "selected": n,
                                    "reason": msg,
                                    "backend": self.decisions.backend_name()
                                }),
                            );
                            let short = |s: &str| {
                                s.split('/')
                                    .next_back()
                                    .unwrap_or(s)
                                    .trim_end_matches(":free")
                                    .to_string()
                            };
                            self.notice(
                                &format!(
                                    "{} is busy ({msg}). Trying {}…",
                                    short(&self.model),
                                    short(&n)
                                ),
                                false,
                            );
                            self.model = n.clone();
                            tried_models.push(n);
                            self.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"builtin",
                                "update":{"sessionUpdate":"config_option_update","configOptions":self.config_options()}}}));
                        }
                        _ => {
                            let hint = if status == Some(429) {
                                "The free models' hourly limit was reached (200 requests per hour). Try again a bit later."
                            } else {
                                "Could not reach the free model service. Check your internet connection and try again."
                            };
                            self.notice(&format!("{hint} ({msg})"), true);
                            return "end_turn";
                        }
                    }
                }
            }
        }
        self.notice("Stopped after many steps. Ask again to continue.", false);
        "max_turn_requests"
    }

    /// One model call. Ok(true) = tools ran, call the model again.
    fn step(&mut self) -> Result<bool, Step> {
        let mut messages = vec![json!({"role":"system","content":self.system_prompt()})];
        messages.extend(self.history.iter().cloned());
        let body = json!({
            "model": self.model,
            "messages": messages,
            "tools": tools_schema(),
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        let resp = self
            .http
            .post(format!("{}/chat/completions", gateway()))
            .header("Content-Type", "application/json")
            .header("HTTP-Referer", "https://github.com/Ziggyware/termed")
            .header("X-Title", "termed")
            .send(body.to_string())
            .map_err(|e| Step::Failed(e.to_string(), None))?;
        let status = resp.status().as_u16();
        if status != 200 {
            let text = resp.into_body().read_to_string().unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| {
                    v.pointer("/error/message")
                        .and_then(Value::as_str)
                        .map(String::from)
                })
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(Step::Failed(msg, Some(status)));
        }
        let reader = BufReader::new(resp.into_body().into_reader());
        let mut content = String::new();
        // Tool calls arrive in pieces, keyed by index.
        let mut calls: Vec<(String, String, String)> = Vec::new();
        for line in reader.lines() {
            if self.cancel.load(Ordering::SeqCst) {
                return Err(Step::Cancelled);
            }
            let Ok(line) = line else {
                return Err(Step::Failed("connection lost".into(), None));
            };
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                break;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            if let Some(err) = v.get("error") {
                let msg = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("model error")
                    .to_string();
                let code = err.get("code").and_then(Value::as_u64).map(|c| c as u16);
                return Err(Step::Failed(msg, code.or(Some(502))));
            }
            if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
                let used = u.get("total_tokens").and_then(Value::as_u64).unwrap_or(0);
                self.update(json!({"sessionUpdate":"usage_update","used":used,"size":0,"cost":{"amount":0,"currency":"USD"}}));
            }
            let Some(delta) = v.pointer("/choices/0/delta") else {
                continue;
            };
            if let Some(r) = delta
                .get("reasoning")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                self.update(json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":r}}));
            }
            if let Some(c) = delta
                .get("content")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                content.push_str(c);
                self.update(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":c}}));
            }
            for tc in delta
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let i = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                while calls.len() <= i {
                    calls.push(Default::default());
                }
                if let Some(id) = tc.get("id").and_then(Value::as_str) {
                    calls[i].0 = id.to_string();
                }
                if let Some(n) = tc.pointer("/function/name").and_then(Value::as_str) {
                    calls[i].1.push_str(n);
                }
                if let Some(a) = tc.pointer("/function/arguments").and_then(Value::as_str) {
                    calls[i].2.push_str(a);
                }
            }
        }
        calls.retain(|c| !c.1.is_empty());
        let mut assistant = json!({"role":"assistant","content":content});
        if !calls.is_empty() {
            for (k, c) in calls.iter_mut().enumerate() {
                if c.0.is_empty() {
                    c.0 = format!("call_{}_{k}", self.next_id);
                }
            }
            assistant["tool_calls"] = Value::Array(
                calls
                    .iter()
                    .map(|(id, name, args)| json!({"id":id,"type":"function","function":{"name":name,"arguments":args}}))
                    .collect(),
            );
        }
        self.history.push(assistant);
        if calls.is_empty() {
            return Ok(false);
        }
        for (id, name, args) in calls {
            if self.cancel.load(Ordering::SeqCst) {
                return Err(Step::Cancelled);
            }
            let args: Value = serde_json::from_str(&args).unwrap_or(json!({}));
            let result = self.run_tool(&id, &name, &args);
            self.history
                .push(json!({"role":"tool","tool_call_id":id,"content":result}));
        }
        Ok(true)
    }

    /// Resolve a model-supplied path inside the project (never outside it).
    fn resolve(&self, p: &str) -> Result<PathBuf, String> {
        let rel = Path::new(p.trim());
        let joined = if rel.is_absolute() {
            rel.to_path_buf()
        } else {
            self.cwd.join(rel)
        };
        let mut clean = PathBuf::new();
        for c in joined.components() {
            match c {
                Component::ParentDir => {
                    clean.pop();
                }
                Component::CurDir => {}
                other => clean.push(other.as_os_str()),
            }
        }
        if clean.starts_with(&self.cwd) {
            Ok(clean)
        } else {
            Err(format!("{p} is outside the project"))
        }
    }

    fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.cwd).unwrap_or(p).display().to_string()
    }

    fn tool_start(&self, id: &str, title: &str, kind: &str, path: Option<&Path>) {
        let locs: Vec<Value> = path
            .map(|p| json!({"path":p.display().to_string()}))
            .into_iter()
            .collect();
        self.update(json!({"sessionUpdate":"tool_call","toolCallId":id,"title":title,"kind":kind,"status":"in_progress","locations":locs}));
        self.agui.emit(
            "tool:started",
            "builtin",
            json!({
                "tool_id": id,
                "title": title,
                "kind": kind,
                "path": path.map(|p| p.display().to_string()),
            }),
        );
    }

    fn tool_end(&self, id: &str, ok: bool, content: Vec<Value>) {
        self.update(json!({"sessionUpdate":"tool_call_update","toolCallId":id,
            "status": if ok {"completed"} else {"failed"},"content":content}));
        self.agui.emit(
            "tool:completed",
            "builtin",
            json!({
                "tool_id": id,
                "status": if ok { "completed" } else { "failed" },
            }),
        );
    }

    fn text_block(s: &str) -> Value {
        json!({"type":"content","content":{"type":"text","text":s}})
    }

    fn run_tool(&mut self, id: &str, name: &str, a: &Value) -> String {
        let s = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let r = match name {
            "list_files" => self.list_files(id, &s("path")),
            "read_file" => self.read_file(id, &s("path"), a),
            "search" => self.search(id, &s("pattern"), &s("path")),
            "edit_file" => self.edit_file(id, &s("path"), &s("old_text"), &s("new_text")),
            "write_file" => self.write_file(id, &s("path"), &s("content")),
            "run_command" => self.run_command(id, &s("command")),
            other => {
                self.tool_start(id, other, "other", None);
                Err(format!("unknown tool {other}"))
            }
        };
        match r {
            Ok(out) => out,
            Err(e) => {
                self.tool_end(id, false, vec![Self::text_block(&e)]);
                format!("Error: {e}")
            }
        }
    }

    fn list_files(&self, id: &str, path: &str) -> Result<String, String> {
        let dir = self.resolve(if path.is_empty() { "." } else { path })?;
        self.tool_start(
            id,
            &format!("List {}", if path.is_empty() { "." } else { path }),
            "read",
            None,
        );
        let mut files = Vec::new();
        for e in ignore::WalkBuilder::new(&dir)
            .hidden(false)
            .build()
            .flatten()
        {
            if e.file_name() == ".git" || !e.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            files.push(self.rel(e.path()));
            if files.len() >= 400 {
                files.push("… (truncated)".into());
                break;
            }
        }
        files.sort();
        let out = files.join("\n");
        self.tool_end(
            id,
            true,
            vec![Self::text_block(&format!("{} files", files.len()))],
        );
        Ok(if out.is_empty() {
            "(no files)".into()
        } else {
            out
        })
    }

    fn read_file(&self, id: &str, path: &str, a: &Value) -> Result<String, String> {
        let p = self.resolve(path)?;
        self.tool_start(id, &format!("Read {}", self.rel(&p)), "read", Some(&p));
        let text = std::fs::read_to_string(&p).map_err(|e| format!("cannot read {path}: {e}"))?;
        let start = a
            .get("start_line")
            .and_then(Value::as_u64)
            .map(|n| n.max(1) as usize);
        let end = a
            .get("end_line")
            .and_then(Value::as_u64)
            .map(|n| n as usize);
        let lines: Vec<&str> = text.lines().collect();
        let (s, e) = (
            start.unwrap_or(1),
            end.unwrap_or(lines.len()).min(lines.len()),
        );
        let mut body: String = lines
            .get(s.saturating_sub(1)..e.max(s.saturating_sub(1)))
            .unwrap_or(&[])
            .join("\n");
        if body.len() > 60_000 {
            body.truncate(60_000);
            body.push_str("\n… (truncated; read a line range)");
        }
        self.tool_end(id, true, vec![]);
        Ok(body)
    }

    fn search(&self, id: &str, pattern: &str, path: &str) -> Result<String, String> {
        let re = regex::Regex::new(pattern).map_err(|e| format!("bad pattern: {e}"))?;
        let dir = self.resolve(if path.is_empty() { "." } else { path })?;
        self.tool_start(id, &format!("Search \"{pattern}\""), "search", None);
        let mut hits = Vec::new();
        'files: for e in ignore::WalkBuilder::new(&dir).build().flatten() {
            if !e.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else {
                continue;
            };
            for (n, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    let l: String = line.trim().chars().take(200).collect();
                    hits.push(format!("{}:{}: {l}", self.rel(e.path()), n + 1));
                    if hits.len() >= 100 {
                        break 'files;
                    }
                }
            }
        }
        self.tool_end(
            id,
            true,
            vec![Self::text_block(&format!("{} matches", hits.len()))],
        );
        Ok(if hits.is_empty() {
            "No matches.".into()
        } else {
            hits.join("\n")
        })
    }

    fn edit_file(&self, id: &str, path: &str, old: &str, new: &str) -> Result<String, String> {
        let p = self.resolve(path)?;
        let rel = self.rel(&p);
        self.tool_start(id, &format!("Edit {rel}"), "edit", Some(&p));
        let text = std::fs::read_to_string(&p).map_err(|e| format!("cannot read {path}: {e}"))?;
        // Tolerate CRLF files when the model sends LF snippets.
        let crlf = text.contains("\r\n");
        let (old_n, new_n) = if crlf {
            (old.replace('\n', "\r\n"), new.replace('\n', "\r\n"))
        } else {
            (old.into(), new.into())
        };
        let count = if old_n.is_empty() {
            0
        } else {
            text.matches(old_n.as_str()).count()
        };
        if count != 1 {
            return Err(format!(
                "old_text matched {count} times in {rel}; it must match exactly once (copy it exactly from read_file and add more context)"
            ));
        }
        let updated = text.replacen(old_n.as_str(), &new_n, 1);
        std::fs::write(&p, &updated).map_err(|e| e.to_string())?;

        // Structured AEP patch tracking & AG-UI emission
        let patch = AepPatch::from_replace(&rel, old, new);
        self.agui.emit(
            "patch:applied",
            "builtin",
            json!({
                "path": rel,
                "old_text": old,
                "new_text": new,
                "hunk_count": patch.hunks.len(),
                "protocol": "AEP"
            }),
        );

        // Jev-Judge post-edit verification (ask_noul)
        let judge_ctx = DecisionContext {
            task_id: format!("edit_judge_{id}"),
            working_dir: self.cwd.to_string_lossy().into_owned(),
            session_history_summary: String::new(),
            metadata: json!({ "path": rel, "lines": updated.lines().count() }),
        };
        if let Ok(res) = self.decisions.ask_noul("Did the edit apply cleanly and keep document valid?", &judge_ctx) {
            self.agui.emit(
                "decision:evaluated",
                "builtin",
                json!({
                    "query": "jev-judge: verify edit",
                    "judgment": res.judgment,
                    "confidence": res.confidence,
                    "margin": res.margin,
                    "rationale": res.rationale,
                    "backend": self.decisions.backend_name()
                }),
            );
        }

        self.tool_end(
            id,
            true,
            vec![json!({"type":"diff","path":p.display().to_string(),"oldText":old,"newText":new})],
        );
        Ok(format!("Edited {rel}."))
    }

    fn write_file(&self, id: &str, path: &str, content: &str) -> Result<String, String> {
        let p = self.resolve(path)?;
        let rel = self.rel(&p);
        let old = std::fs::read_to_string(&p).ok();
        self.tool_start(
            id,
            &format!("{} {rel}", if old.is_some() { "Write" } else { "Create" }),
            "edit",
            Some(&p),
        );
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&p, content).map_err(|e| e.to_string())?;
        self.tool_end(id, true, vec![json!({"type":"diff","path":p.display().to_string(),"oldText":old,"newText":content})]);
        Ok(format!("Wrote {rel} ({} lines).", content.lines().count()))
    }

    /// Ask the user (through the panel) to approve something.
    fn permission(&mut self, tool_id: &str, title: &str) -> bool {
        if self.always_allow_commands {
            return true;
        }
        self.next_id += 1;
        let rid = self.next_id;
        self.send(
            json!({"jsonrpc":"2.0","id":rid,"method":"session/request_permission","params":{
            "sessionId":"builtin","toolCall":{"toolCallId":tool_id,"title":title},
            "options":[{"optionId":"once","name":"Allow","kind":"allow_once"},
                       {"optionId":"always","name":"Always allow","kind":"allow_always"},
                       {"optionId":"no","name":"Reject","kind":"reject_once"}]}}),
        );
        loop {
            if self.cancel.load(Ordering::SeqCst) {
                return false;
            }
            match self.replies.recv_timeout(Duration::from_millis(200)) {
                Ok(v) if v.get("id").and_then(Value::as_u64) == Some(rid) => {
                    let choice = v
                        .pointer("/result/outcome/optionId")
                        .and_then(Value::as_str)
                        .unwrap_or("no");
                    if choice == "always" {
                        self.always_allow_commands = true;
                    }
                    return choice != "no";
                }
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return false,
            }
        }
    }

    fn run_command(&mut self, id: &str, command: &str) -> Result<String, String> {
        self.tool_start(id, &format!("Run `{command}`"), "execute", None);

        // Jev-governed permission risk evaluation
        let verdict = self.policy.evaluate_command(command, &self.cwd.to_string_lossy());
        self.agui.emit(
            "decision:evaluated",
            "builtin",
            json!({
                "metric": "shell_execution_risk",
                "command": command,
                "backend": self.decisions.backend_name(),
                "verdict": match &verdict {
                    PermissionVerdict::AutoAllow { score, rationale } => json!({"type": "AutoAllow", "score": score, "rationale": rationale}),
                    PermissionVerdict::PromptUser { score, rationale } => json!({"type": "PromptUser", "score": score, "rationale": rationale}),
                    PermissionVerdict::Block { score, reason } => json!({"type": "Block", "score": score, "reason": reason}),
                }
            }),
        );

        match verdict {
            PermissionVerdict::AutoAllow { .. } => {
                // Low risk command auto-allowed (e.g. read/test commands)
            }
            PermissionVerdict::Block { score, reason } => {
                self.tool_end(id, false, vec![Self::text_block(&format!("Blocked by policy (risk {:.2}): {reason}", score))]);
                return Ok(format!("Command blocked by Jev risk policy: {reason}"));
            }
            PermissionVerdict::PromptUser { score, rationale } => {
                let prompt_desc = format!("run `{command}` (risk {:.2}: {})", score, rationale);
                if !self.permission(id, &prompt_desc) {
                    self.tool_end(id, false, vec![Self::text_block("Rejected by the user.")]);
                    return Ok("The user rejected this command.".into());
                }
            }
        }
        let started = Instant::now();
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run: {e}"))?;
        // Collect output on threads so a chatty command can't block on a full pipe.
        let take = |r: Option<Box<dyn std::io::Read + Send>>| {
            std::thread::spawn(move || {
                let mut s = String::new();
                if let Some(mut r) = r {
                    let _ = r.read_to_string(&mut s);
                }
                s
            })
        };
        let out_t = take(
            child
                .stdout
                .take()
                .map(|r| Box::new(r) as Box<dyn std::io::Read + Send>),
        );
        let err_t = take(
            child
                .stderr
                .take()
                .map(|r| Box::new(r) as Box<dyn std::io::Read + Send>),
        );
        let status = loop {
            if let Ok(Some(s)) = child.try_wait() {
                break Some(s);
            }
            if self.cancel.load(Ordering::SeqCst) || started.elapsed() > Duration::from_secs(120) {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let mut out = out_t.join().unwrap_or_default();
        out.push_str(&err_t.join().unwrap_or_default());
        if out.len() > 8000 {
            out = format!("…{}", &out[out.len() - 8000..]);
        }
        let code = match status {
            Some(s) => format!("exit code {}", s.code().unwrap_or(-1)),
            None => "stopped (timeout or cancelled)".into(),
        };
        let shown: String = out
            .lines()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        self.tool_end(
            id,
            status.is_some_and(|s| s.success()),
            vec![Self::text_block(&format!("{code}\n{shown}"))],
        );
        Ok(format!("{code}\n{out}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(dir: &Path) -> Worker {
        let (_tx, rx) = channel();
        Worker::new(
            dir.to_path_buf(),
            Arc::new(|_| {}),
            rx,
            Arc::new(AtomicBool::new(false)),
        )
    }

    #[test]
    fn tools_edit_and_stay_inside_the_project() {
        let dir = std::env::temp_dir().join(format!("termed-builtin-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.py"), "x = 1\ny = 2\n").unwrap();
        let mut w = worker(&dir);
        assert!(w.resolve("../../etc/passwd").is_err());
        assert!(w.resolve("src/../src/a.py").is_ok());
        let r = w.run_tool(
            "1",
            "edit_file",
            &json!({"path":"src/a.py","old_text":"y = 2","new_text":"y = 3"}),
        );
        assert_eq!(r, "Edited src/a.py.");
        assert_eq!(
            std::fs::read_to_string(dir.join("src/a.py")).unwrap(),
            "x = 1\ny = 3\n"
        );
        let r = w.run_tool(
            "2",
            "edit_file",
            &json!({"path":"src/a.py","old_text":"nope","new_text":"z"}),
        );
        assert!(r.starts_with("Error: old_text matched 0 times"));
        let r = w.run_tool("3", "search", &json!({"pattern":"y = \\d"}));
        assert_eq!(r, "src/a.py:2: y = 3");
        let r = w.run_tool(
            "4",
            "read_file",
            &json!({"path":"src/a.py","start_line":2,"end_line":2}),
        );
        assert_eq!(r, "y = 3");
        let r = w.run_tool(
            "5",
            "write_file",
            &json!({"path":"new/b.txt","content":"hi\n"}),
        );
        assert!(r.starts_with("Wrote new/b.txt"));
        assert!(
            w.run_tool("6", "list_files", &json!({}))
                .contains("new/b.txt")
        );
    }
}
