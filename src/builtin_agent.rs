//! termed's built-in coding agent. It needs no install and, with the default
//! provider, no account or API key.
//!
//! It runs a small tool-using agent loop (list, read, search, edit and create
//! files, and run commands, each command with your approval) against a model
//! provider from [`crate::providers`]. By default that is Kilo Gateway's free
//! models (anonymous, rate-limited per IP). Ollama, OpenRouter, Gemini, Groq,
//! Cerebras, Mistral, OpenAI, DeepSeek, xAI or any OpenAI-compatible server
//! work the same way, and Claude goes through Anthropic's native Messages API.
//!
//! It is pure Rust, so it works anywhere termed runs, including Termux on
//! Android, where Node/Bun-based agents can't be installed. It speaks the same
//! Agent Client Protocol messages as an external agent, only over in-process
//! channels, so the agent panel treats it like any other ACP agent.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::providers::{self, Api, Provider};

/// Kilo's router across its free models.
pub const DEFAULT_MODEL: &str = "kilo-auto/free";
const MAX_STEPS: usize = 30;
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Server-side refusal fallbacks (`"fallbacks": "default"`).
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// Start the agent thread for `provider` (an id from [`providers::PROVIDERS`]).
/// `out` receives the JSON-RPC messages an external agent would print; the
/// returned sender takes the messages we'd write to it.
pub fn spawn(
    cwd: PathBuf,
    provider: &str,
    out: impl Fn(Value) + Send + Sync + 'static,
) -> Sender<Value> {
    let provider = providers::get(provider).unwrap_or(&providers::PROVIDERS[0]);
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
        let mut w = Worker::new(cwd, provider, out, reply_rx, cancel);
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

/// What the Anthropic models endpoint says a Claude model supports.
#[derive(Clone, Copy, Debug)]
struct ClaudeCaps {
    adaptive: bool,
    effort_high: bool,
    max_out: u64,
}

impl ClaudeCaps {
    /// Used when the model isn't in the listing (e.g. set by hand).
    fn guess(model: &str) -> Self {
        let modern = [
            "opus-5",
            "fable-5",
            "sonnet-5",
            "opus-4-6",
            "opus-4-7",
            "opus-4-8",
            "sonnet-4-6",
        ]
        .iter()
        .any(|m| model.contains(m));
        Self {
            adaptive: modern,
            effort_high: modern,
            max_out: 64_000,
        }
    }
}

/// Models that accept `"fallbacks": "default"`.
fn supports_fallbacks(model: &str) -> bool {
    ["claude-opus-5", "claude-fable-5-1", "claude-sonnet-5-5"]
        .iter()
        .any(|m| model.starts_with(m))
}

fn get_json(
    http: &ureq::Agent,
    url: &str,
    headers: &[(&str, String)],
) -> Result<Value, (String, Option<u16>)> {
    let mut req = http.get(url);
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let resp = req.call().map_err(|e| (e.to_string(), None))?;
    let status = resp.status().as_u16();
    let text = resp.into_body().read_to_string().unwrap_or_default();
    if status != 200 {
        return Err((providers::error_message(&text, status), Some(status)));
    }
    serde_json::from_str(&text).map_err(|e| (e.to_string(), Some(status)))
}

fn tool_specs() -> Vec<(&'static str, &'static str, Value, &'static [&'static str])> {
    vec![
        (
            "list_files",
            "List files in a directory of the project (recursive, respects .gitignore).",
            json!({"path":{"type":"string","description":"Directory relative to the project root; default '.'"}}),
            &[],
        ),
        (
            "read_file",
            "Read a text file. Optionally a line range (1-based, inclusive).",
            json!({"path":{"type":"string"},"start_line":{"type":"integer"},"end_line":{"type":"integer"}}),
            &["path"],
        ),
        (
            "search",
            "Search file contents with a regular expression. Returns file:line: text.",
            json!({"pattern":{"type":"string"},"path":{"type":"string","description":"Directory to search; default '.'"}}),
            &["pattern"],
        ),
        (
            "edit_file",
            "Replace old_text with new_text in a file. old_text must match exactly once; include enough surrounding lines to make it unique.",
            json!({"path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"}}),
            &["path", "old_text", "new_text"],
        ),
        (
            "write_file",
            "Create a file or overwrite it completely.",
            json!({"path":{"type":"string"},"content":{"type":"string"}}),
            &["path", "content"],
        ),
        (
            "run_command",
            "Run a shell command in the project root (the user must approve it). Use for builds and tests.",
            json!({"command":{"type":"string"}}),
            &["command"],
        ),
    ]
}

/// OpenAI-style function tools.
fn openai_tools() -> Value {
    tool_specs()
        .into_iter()
        .map(|(name, desc, props, req)| {
            json!({"type":"function","function":{"name":name,"description":desc,
                "parameters":{"type":"object","properties":props,"required":req}}})
        })
        .collect()
}

/// Anthropic tools. Inputs stream as they are generated, so every input is
/// parsed strictly and checked with [`validate_input`] before a tool runs.
fn anthropic_tools() -> Value {
    tool_specs()
        .into_iter()
        .map(|(name, desc, props, req)| {
            json!({"name":name,"description":desc,"eager_input_streaming":true,
                "input_schema":{"type":"object","properties":props,"required":req}})
        })
        .collect()
}

/// Check a tool input against its schema: an object, required keys present,
/// and every known key of the declared type.
fn validate_input(name: &str, input: &Value) -> Result<(), String> {
    let Some(obj) = input.as_object() else {
        return Err("input is not a JSON object".into());
    };
    let Some((_, _, props, req)) = tool_specs().into_iter().find(|t| t.0 == name) else {
        return Err(format!("unknown tool {name}"));
    };
    for r in req {
        if !obj.contains_key(*r) {
            return Err(format!("missing required field {r}"));
        }
    }
    for (k, v) in obj {
        let ok = match props.pointer(&format!("/{k}/type")).and_then(Value::as_str) {
            Some("string") => v.is_string(),
            Some("integer") => v.is_u64() || v.is_i64(),
            _ => true,
        };
        if !ok {
            return Err(format!("field {k} has the wrong type"));
        }
    }
    Ok(())
}

/// Drop the blocks a mid-output refusal fallback made stale: thinking,
/// redacted thinking, tool calls and unknown internal blocks before the last
/// `fallback` block. Text, the fallback markers and everything after stay.
fn strip_before_fallback(blocks: Vec<Value>) -> Vec<Value> {
    let ty = |b: &Value| {
        b.get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let Some(last) = blocks.iter().rposition(|b| ty(b) == "fallback") else {
        return blocks;
    };
    blocks
        .into_iter()
        .enumerate()
        .filter(|(i, b)| *i >= last || matches!(ty(b).as_str(), "text" | "fallback"))
        .map(|(_, b)| b)
        .collect()
}

struct Worker {
    cwd: PathBuf,
    provider: &'static Provider,
    key: Option<String>,
    out: Arc<dyn Fn(Value) + Send + Sync>,
    replies: Receiver<Value>,
    cancel: Arc<AtomicBool>,
    http: ureq::Agent,
    model: String,
    models: Vec<(String, String)>,
    /// Model ids that cost nothing (so busy ones can be swapped).
    free_models: Vec<String>,
    claude_caps: HashMap<String, ClaudeCaps>,
    /// Conversation in the provider's own message format.
    history: Vec<Value>,
    next_id: u64,
    always_allow_commands: bool,
}

enum Step {
    Cancelled,
    Failed(String, Option<u16>),
    /// The model declined or was cut off; the turn ends with this notice.
    Stopped(String),
}

impl Worker {
    fn new(
        cwd: PathBuf,
        provider: &'static Provider,
        out: Arc<dyn Fn(Value) + Send + Sync>,
        replies: Receiver<Value>,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        let env_model = if provider.id == "kilo" {
            std::env::var("TERMED_FREE_MODEL").ok()
        } else {
            std::env::var("TERMED_MODEL").ok()
        };
        Self {
            cwd,
            provider,
            key: provider.api_key(),
            out,
            replies,
            cancel,
            http: http(),
            model: env_model.unwrap_or_default(),
            models: Vec::new(),
            free_models: Vec::new(),
            claude_caps: HashMap::new(),
            history: Vec::new(),
            next_id: 1000,
            always_allow_commands: false,
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

    fn base(&self) -> String {
        self.provider.base_url()
    }

    fn auth_headers(&self) -> Vec<(&'static str, String)> {
        let mut h = Vec::new();
        match self.provider.api {
            Api::OpenAi => {
                if let Some(k) = &self.key {
                    h.push(("Authorization", format!("Bearer {k}")));
                }
                h.push(("HTTP-Referer", "https://github.com/Ziggyware/termed".into()));
                h.push(("X-Title", "termed".into()));
            }
            Api::Anthropic => {
                if let Some(k) = &self.key {
                    h.push(("x-api-key", k.clone()));
                }
                h.push(("anthropic-version", ANTHROPIC_VERSION.into()));
            }
        }
        h
    }

    /// Fill `models` from the provider; returns a problem to show, if any.
    fn load_models(&mut self) -> Option<String> {
        let p = self.provider;
        if p.needs_key && self.key.is_none() {
            return Some(self.key_hint());
        }
        let url = match p.api {
            Api::OpenAi => format!("{}/models", self.base()),
            Api::Anthropic => format!("{}/v1/models?limit=100", self.base()),
        };
        let listing = get_json(&self.http, &url, &self.auth_headers());
        let mut models = Vec::new();
        let mut free = Vec::new();
        let problem = match listing {
            Ok(v) => {
                for m in v
                    .get("data")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let id = m
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = m
                        .get("display_name")
                        .or(m.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .to_string();
                    let tools = m.get("supported_parameters").and_then(Value::as_array);
                    let has_tools = tools.is_none_or(|t| t.iter().any(|x| x == "tools"));
                    match p.id {
                        // Only the anonymous free models work on Kilo without a key.
                        "kilo" if self.key.is_none() => {
                            if id.ends_with(":free") && has_tools {
                                free.push(id.clone());
                                models.push((id, name));
                            }
                        }
                        "kilo" | "openrouter" => {
                            if has_tools {
                                if id.ends_with(":free") {
                                    free.push(id.clone());
                                }
                                models.push((id, name));
                            }
                        }
                        _ if p.api == Api::Anthropic => {
                            let caps = m.get("capabilities");
                            let leaf = |path: &str| {
                                caps.and_then(|c| c.pointer(path))
                                    .and_then(|s| s.get("supported"))
                                    .and_then(Value::as_bool)
                            };
                            let guess = ClaudeCaps::guess(&id);
                            self.claude_caps.insert(
                                id.clone(),
                                ClaudeCaps {
                                    adaptive: leaf("/thinking/types/adaptive")
                                        .unwrap_or(guess.adaptive),
                                    effort_high: leaf("/effort/high").unwrap_or(guess.effort_high),
                                    max_out: m
                                        .get("max_tokens")
                                        .and_then(Value::as_u64)
                                        .unwrap_or(guess.max_out),
                                },
                            );
                            models.push((id, name));
                        }
                        _ => {
                            if providers::is_chat_model(&id) {
                                models.push((id.trim_start_matches("models/").to_string(), name));
                            }
                        }
                    }
                }
                // Free ones first, the rest sorted.
                models.sort_by_key(|(id, _)| (!free.contains(id), id.clone()));
                None
            }
            Err((msg, status)) => Some(self.explain(&msg, status)),
        };
        // Routers that pick a model for you.
        match p.id {
            "kilo" => {
                models.insert(0, (DEFAULT_MODEL.into(), "Auto (best free model)".into()));
                free.insert(0, DEFAULT_MODEL.into());
            }
            "openrouter" => {
                models.insert(
                    0,
                    ("openrouter/free".into(), "Auto (best free model)".into()),
                );
                free.insert(0, "openrouter/free".into());
            }
            _ => {}
        }
        if self.model.is_empty() {
            self.model = p.pick_model(&models);
        }
        if !self.model.is_empty() && !models.iter().any(|(v, _)| *v == self.model) {
            models.insert(0, (self.model.clone(), self.model.clone()));
        }
        self.models = models;
        self.free_models = free;
        if self.model.is_empty() && problem.is_none() {
            return Some(match p.id {
                "ollama" => {
                    "Ollama has no models yet. Pull one first, e.g. `ollama pull qwen2.5-coder`."
                        .into()
                }
                _ => format!("{} offered no models.", p.name),
            });
        }
        problem
    }

    fn key_hint(&self) -> String {
        let p = self.provider;
        let env = p.key_envs.first().copied().unwrap_or("");
        format!(
            "{} needs an API key. Get one at {} and paste it with the command \"Agent: Set API Key\" (or set {env}).",
            p.name, p.key_url
        )
    }

    /// Turn an HTTP failure into advice.
    fn explain(&self, msg: &str, status: Option<u16>) -> String {
        let p = self.provider;
        match status {
            Some(401 | 403) if p.needs_key || self.key.is_some() => {
                format!(
                    "{} rejected the API key ({msg}). Paste a valid one with \"Agent: Set API Key\" (get keys at {}).",
                    p.name, p.key_url
                )
            }
            Some(401 | 403) => format!("{} refused the request ({msg}).", p.name),
            Some(429) if p.id == "kilo" => format!(
                "The free models' hourly limit was reached (200 requests per hour). Try again a bit later. ({msg})"
            ),
            Some(429) => format!(
                "{} rate limit reached; wait a moment and try again. ({msg})",
                p.name
            ),
            None if p.id == "ollama" => format!(
                "Can't reach Ollama at {}. Start it with `ollama serve` (Termux: pkg install ollama). ({msg})",
                self.base()
            ),
            None => format!(
                "Could not reach {}. Check your internet connection and try again. ({msg})",
                p.name
            ),
            Some(_) => format!("{}: {msg}", p.name),
        }
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
                    "agentInfo":{"name":"termed","title":if self.provider.id == "kilo" { "termed AI".to_string() } else { self.provider.name.to_string() },
                        "version":env!("CARGO_PKG_VERSION")}}),
            ),
            "session/new" => {
                if let Some(c) = p.get("cwd").and_then(Value::as_str) {
                    self.cwd = PathBuf::from(c);
                }
                self.history.clear();
                let problem = if self.models.is_empty() { self.load_models() } else { None };
                self.respond(&id, json!({"sessionId":"builtin","configOptions":self.config_options()}));
                if let Some(problem) = problem {
                    self.notice(&problem, true);
                }
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

    /// Append a user message, merging into a trailing user message (left
    /// behind by a refusal) so roles keep alternating.
    fn push_user(&mut self, content: Value) {
        let anthropic = self.provider.api == Api::Anthropic;
        if anthropic
            && let Some(last) = self.history.last_mut()
            && last["role"] == "user"
        {
            let as_blocks = |c: Value| match c {
                Value::String(s) => vec![json!({"type":"text","text":s})],
                Value::Array(a) => a,
                other => vec![other],
            };
            let mut blocks = as_blocks(last["content"].take());
            blocks.extend(as_blocks(content));
            last["content"] = Value::Array(blocks);
            return;
        }
        self.history.push(json!({"role":"user","content":content}));
    }

    /// One user turn: loop model → tools until the model stops calling tools.
    fn turn(&mut self, p: &Value) -> &'static str {
        if self.provider.needs_key && self.key.is_none() {
            // A key may have been saved since this agent started.
            self.key = self.provider.api_key();
            if self.key.is_none() {
                self.notice(&self.key_hint(), true);
                return "end_turn";
            }
        }
        if self.model.is_empty() || self.models.is_empty() {
            if let Some(problem) = self.load_models() {
                self.notice(&problem, true);
                if self.model.is_empty() {
                    return "end_turn";
                }
            }
            self.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"builtin",
                "update":{"sessionUpdate":"config_option_update","configOptions":self.config_options()}}}));
        }
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
        self.push_user(json!(text.trim()));
        let mut tried_models = vec![self.model.clone()];
        for _ in 0..MAX_STEPS {
            if self.cancel.load(Ordering::SeqCst) {
                return "cancelled";
            }
            let r = match self.provider.api {
                Api::OpenAi => self.openai_step(),
                Api::Anthropic => self.anthropic_step(),
            };
            match r {
                Ok(true) => continue,
                Ok(false) => return "end_turn",
                Err(Step::Cancelled) => return "cancelled",
                Err(Step::Stopped(msg)) => {
                    self.notice(&msg, false);
                    return "end_turn";
                }
                Err(Step::Failed(msg, status)) => {
                    // A busy or failing free model: move on to another free one.
                    let busy = matches!(status, Some(429) | Some(500..=599)) || status.is_none();
                    let next = self
                        .free_models
                        .iter()
                        .find(|v| !tried_models.contains(v))
                        .cloned();
                    match next {
                        Some(n)
                            if busy
                                && self.free_models.contains(&self.model)
                                && tried_models.len() < 4 =>
                        {
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
                            self.notice(&self.explain(&msg, status), true);
                            return "end_turn";
                        }
                    }
                }
            }
        }
        self.notice("Stopped after many steps. Ask again to continue.", false);
        "max_turn_requests"
    }

    fn post(
        &self,
        url: &str,
        body: &Value,
        extra: &[(&str, &str)],
    ) -> Result<ureq::http::Response<ureq::Body>, Step> {
        let mut req = self
            .http
            .post(url)
            .header("Content-Type", "application/json");
        for (k, v) in self.auth_headers() {
            req = req.header(k, v);
        }
        for (k, v) in extra {
            req = req.header(*k, *v);
        }
        let resp = req
            .send(body.to_string())
            .map_err(|e| Step::Failed(e.to_string(), None))?;
        let status = resp.status().as_u16();
        if status != 200 {
            let text = resp.into_body().read_to_string().unwrap_or_default();
            return Err(Step::Failed(
                providers::error_message(&text, status),
                Some(status),
            ));
        }
        Ok(resp)
    }

    fn usage(&self, used: u64) {
        // Only free models report a cost (zero); paid usage shows tokens only.
        let mut u = json!({"sessionUpdate":"usage_update","used":used,"size":0});
        if self.free_models.contains(&self.model) || self.provider.id == "ollama" {
            u["cost"] = json!({"amount":0,"currency":"USD"});
        }
        self.update(u);
    }

    /// One OpenAI-compatible model call. Ok(true) = tools ran, call again.
    fn openai_step(&mut self) -> Result<bool, Step> {
        let mut messages = vec![json!({"role":"system","content":self.system_prompt()})];
        messages.extend(self.history.iter().cloned());
        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "tools": openai_tools(),
            "stream": true,
        });
        // Not every OpenAI-compatible server knows stream_options.
        if !matches!(self.provider.id, "ollama" | "custom") {
            body["stream_options"] = json!({"include_usage": true});
        }
        let resp = self.post(&format!("{}/chat/completions", self.base()), &body, &[])?;
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
                self.usage(u.get("total_tokens").and_then(Value::as_u64).unwrap_or(0));
            }
            let Some(delta) = v.pointer("/choices/0/delta") else {
                continue;
            };
            let reasoning = delta.get("reasoning").or(delta.get("reasoning_content"));
            if let Some(r) = reasoning.and_then(Value::as_str).filter(|s| !s.is_empty()) {
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
                let i = tc
                    .get("index")
                    .and_then(Value::as_u64)
                    .unwrap_or(calls.len() as u64) as usize;
                while calls.len() <= i {
                    calls.push(Default::default());
                }
                if let Some(id) = tc.get("id").and_then(Value::as_str) {
                    calls[i].0 = id.to_string();
                }
                if let Some(n) = tc.pointer("/function/name").and_then(Value::as_str) {
                    calls[i].1.push_str(n);
                }
                // Some servers (Ollama) send arguments as an object, not a string.
                match tc.pointer("/function/arguments") {
                    Some(Value::String(a)) => calls[i].2.push_str(a),
                    Some(a @ Value::Object(_)) => calls[i].2 = a.to_string(),
                    _ => {}
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
            self.next_id += 1;
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
                // Keep the history valid: answer every call before stopping.
                self.history.push(
                    json!({"role":"tool","tool_call_id":id,"content":"Cancelled by the user."}),
                );
                continue;
            }
            let args: Value = if args.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(&args).unwrap_or(json!({}))
            };
            let result = self.run_tool(&id, &name, &args);
            self.history
                .push(json!({"role":"tool","tool_call_id":id,"content":result}));
        }
        if self.cancel.load(Ordering::SeqCst) {
            return Err(Step::Cancelled);
        }
        Ok(true)
    }

    /// One call to Anthropic's Messages API (streamed). Ok(true) = tools ran.
    fn anthropic_step(&mut self) -> Result<bool, Step> {
        let caps = self
            .claude_caps
            .get(&self.model)
            .copied()
            .unwrap_or_else(|| ClaudeCaps::guess(&self.model));
        let mut body = json!({
            "model": self.model,
            "max_tokens": caps.max_out.min(64_000),
            "stream": true,
            "system": self.system_prompt(),
            "tools": anthropic_tools(),
            "messages": self.history,
        });
        if caps.adaptive {
            body["thinking"] = json!({"type":"adaptive","display":"summarized"});
        }
        if caps.effort_high {
            body["output_config"] = json!({"effort":"high"});
        }
        let mut extra = Vec::new();
        if supports_fallbacks(&self.model) {
            body["fallbacks"] = json!("default");
            extra.push(("anthropic-beta", FALLBACK_BETA));
        }
        let resp = self.post(&format!("{}/v1/messages", self.base()), &body, &extra)?;
        let reader = BufReader::new(resp.into_body().into_reader());
        // Content blocks by index; tool inputs accumulate as raw JSON text.
        let mut blocks: Vec<Value> = Vec::new();
        let mut inputs: Vec<String> = Vec::new();
        let mut stop_reason = String::new();
        let (mut input_tokens, mut output_tokens) = (0u64, 0u64);
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
            let Ok(v) = serde_json::from_str::<Value>(data.trim()) else {
                continue;
            };
            let idx = v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            match v.get("type").and_then(Value::as_str).unwrap_or("") {
                "message_start" => {
                    let u = v.pointer("/message/usage");
                    let n = |k: &str| {
                        u.and_then(|u| u.get(k))
                            .and_then(Value::as_u64)
                            .unwrap_or(0)
                    };
                    input_tokens = n("input_tokens")
                        + n("cache_read_input_tokens")
                        + n("cache_creation_input_tokens");
                }
                "content_block_start" => {
                    while blocks.len() <= idx {
                        blocks.push(Value::Null);
                        inputs.push(String::new());
                    }
                    let mut b = v.get("content_block").cloned().unwrap_or(Value::Null);
                    if b["type"] == "tool_use" {
                        // The input arrives as input_json_delta fragments.
                        b["input"] = json!({});
                    }
                    blocks[idx] = b;
                }
                "content_block_delta" => {
                    let Some(b) = blocks.get_mut(idx) else {
                        continue;
                    };
                    let d = v.get("delta").cloned().unwrap_or(Value::Null);
                    let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                    match d.get("type").and_then(Value::as_str).unwrap_or("") {
                        "text_delta" => {
                            let t = s("text");
                            let mut all = b["text"].as_str().unwrap_or("").to_string();
                            all.push_str(&t);
                            b["text"] = json!(all);
                            if !t.is_empty() {
                                self.update(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":t}}));
                            }
                        }
                        "thinking_delta" => {
                            let t = s("thinking");
                            let mut all = b["thinking"].as_str().unwrap_or("").to_string();
                            all.push_str(&t);
                            b["thinking"] = json!(all);
                            if !t.is_empty() {
                                self.update(json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":t}}));
                            }
                        }
                        "signature_delta" => {
                            let mut all = b["signature"].as_str().unwrap_or("").to_string();
                            all.push_str(&s("signature"));
                            b["signature"] = json!(all);
                        }
                        "input_json_delta" => inputs[idx].push_str(&s("partial_json")),
                        _ => {}
                    }
                }
                "message_delta" => {
                    if let Some(r) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                        stop_reason = r.to_string();
                    }
                    if let Some(n) = v.pointer("/usage/output_tokens").and_then(Value::as_u64) {
                        output_tokens = n;
                    }
                }
                "error" => {
                    let msg = v
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("model error")
                        .to_string();
                    let status = match v.pointer("/error/type").and_then(Value::as_str) {
                        Some("overloaded_error") => 529,
                        Some("rate_limit_error") => 429,
                        _ => 500,
                    };
                    return Err(Step::Failed(msg, Some(status)));
                }
                "message_stop" => break,
                _ => {}
            }
        }
        if input_tokens + output_tokens > 0 {
            self.usage(input_tokens + output_tokens);
        }
        if stop_reason == "refusal" {
            // Never run tools from a declined turn, and drop its partial reply.
            return Err(Step::Stopped(
                "Claude declined this request (and no fallback model took it). Try rephrasing."
                    .into(),
            ));
        }
        // Parse each tool input strictly; invalid ones get an error result.
        let mut parsed: Vec<Option<Result<Value, String>>> = Vec::new();
        for (i, b) in blocks.iter_mut().enumerate() {
            if b["type"] != "tool_use" {
                parsed.push(None);
                continue;
            }
            let raw = inputs[i].trim();
            let r = if raw.is_empty() {
                Ok(json!({}))
            } else {
                serde_json::from_str::<Value>(raw)
            };
            parsed.push(Some(match r {
                Ok(input) => {
                    b["input"] = input.clone();
                    let name = b["name"].as_str().unwrap_or("");
                    validate_input(name, &input)
                        .map(|_| input)
                        .map_err(|_| raw.to_string())
                }
                Err(_) => Err(raw.to_string()),
            }));
        }
        // Echo the turn back unchanged (thinking blocks keep their signatures),
        // minus what a mid-output fallback made stale.
        let had_fallback = blocks.iter().any(|b| b["type"] == "fallback");
        let mut content: Vec<Value> = blocks.iter().filter(|b| !b.is_null()).cloned().collect();
        if had_fallback {
            content = strip_before_fallback(content);
        }
        if stop_reason == "max_tokens" {
            // A cut-off tool input can't be trusted: keep the text, run nothing.
            content.retain(|b| b["type"] != "tool_use");
            if !content.is_empty() {
                self.history
                    .push(json!({"role":"assistant","content":content}));
            }
            return Err(Step::Stopped(
                "The reply hit the output limit. Ask again to continue.".into(),
            ));
        }
        let live: Vec<String> = content
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .filter_map(|b| b["id"].as_str().map(String::from))
            .collect();
        if !content.is_empty() {
            self.history
                .push(json!({"role":"assistant","content":content}));
        }
        if live.is_empty() {
            return Ok(false);
        }
        // Run the surviving tool calls; every result goes in one user message.
        let mut results = Vec::new();
        for (b, p) in blocks.iter().zip(parsed) {
            let Some(p) = p else { continue };
            let id = b["id"].as_str().unwrap_or("").to_string();
            if !live.contains(&id) {
                continue;
            }
            if self.cancel.load(Ordering::SeqCst) {
                // Keep the history valid: answer every call before stopping.
                results.push(json!({"type":"tool_result","tool_use_id":id,"is_error":true,"content":"Cancelled by the user."}));
                continue;
            }
            let name = b["name"].as_str().unwrap_or("").to_string();
            results.push(match p {
                Ok(input) => json!({"type":"tool_result","tool_use_id":id,"content":self.run_tool(&id, &name, &input)}),
                Err(raw) => json!({"type":"tool_result","tool_use_id":id,"is_error":true,
                    "content":json!({"INVALID_JSON": raw}).to_string()}),
            });
        }
        self.history.push(json!({"role":"user","content":results}));
        if self.cancel.load(Ordering::SeqCst) {
            return Err(Step::Cancelled);
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
    }

    fn tool_end(&self, id: &str, ok: bool, content: Vec<Value>) {
        self.update(json!({"sessionUpdate":"tool_call_update","toolCallId":id,
            "status": if ok {"completed"} else {"failed"},"content":content}));
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
        std::fs::write(&p, updated).map_err(|e| e.to_string())?;
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
        if !self.permission(id, &format!("run `{command}`")) {
            self.tool_end(id, false, vec![Self::text_block("Rejected by the user.")]);
            return Ok("The user rejected this command.".into());
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
            &providers::PROVIDERS[0],
            Arc::new(|_| {}),
            rx,
            Arc::new(AtomicBool::new(false)),
        )
    }

    #[test]
    fn fallback_echo_drops_stale_blocks_before_the_last_marker() {
        let blocks = vec![
            json!({"type":"thinking","thinking":"a","signature":"s"}),
            json!({"type":"text","text":"partial"}),
            json!({"type":"tool_use","id":"x","name":"read_file","input":{}}),
            json!({"type":"fallback"}),
            json!({"type":"thinking","thinking":"b","signature":"t"}),
            json!({"type":"tool_use","id":"y","name":"read_file","input":{}}),
        ];
        let kept: Vec<String> = strip_before_fallback(blocks)
            .iter()
            .map(|b| b["type"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(kept, ["text", "fallback", "thinking", "tool_use"]);
    }

    #[test]
    fn tool_inputs_are_validated_against_the_schema() {
        assert!(
            validate_input(
                "edit_file",
                &json!({"path":"a","old_text":"b","new_text":"c"})
            )
            .is_ok()
        );
        assert!(validate_input("edit_file", &json!({"path":"a","old_text":"b"})).is_err());
        assert!(validate_input("read_file", &json!({"path":"a","start_line":"1"})).is_err());
        assert!(validate_input("list_files", &json!({})).is_ok());
        assert!(validate_input("list_files", &json!([])).is_err());
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
