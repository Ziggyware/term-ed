//! Authenticated, loopback-only MCP stdio relay for external ACP agents.
//! Requests are handled on the editor thread, so plugins see live buffers and
//! use the same approvals as the built-in agent. No network-facing tool API.
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
};
use std::time::Duration;

use crate::config::AgentConfig;
use crate::mcp::McpTool;
use serde_json::{Value, json};

pub type ToolResult = Result<Value, String>;

pub struct Request {
    pub method: String,
    pub params: Value,
    pub reply: Sender<ToolResult>,
}

pub struct Bridge {
    pub requests: Receiver<Request>,
    pub address: SocketAddr,
    token: String,
    shutdown: Arc<AtomicBool>,
    connections: Arc<std::sync::Mutex<Vec<TcpStream>>>,
}

/// Shared automation tool definitions for the built-in agent and MCP clients.
pub fn tools(options: &AgentConfig) -> Vec<McpTool> {
    let mut tools = vec![
        McpTool { name: "read_editor".into(), description: "Read a live open editor buffer, including unsaved plugin edits. Optional file path, otherwise the active buffer. Returns text and dirty status; use this after running a plugin instead of reading stale disk content.".into(), input_schema: json!({"type":"object","properties":{"file":{"type":"string"}}}) },
        McpTool {
            name: "list_skills".into(),
            description: "List enabled instruction skills available in this workspace.".into(),
            input_schema: json!({"type":"object","properties":{}}),
        },
        McpTool {
            name: "read_skill".into(),
            description: "Read an enabled skill's Markdown instructions before using it.".into(),
            input_schema: json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}),
        },
    ];
    if options.plugins && !options.read_only {
        tools.extend([
            McpTool { name: "list_plugins".into(), description: "List enabled, runnable plugins and their exact command IDs. Plugins execute trusted local code.".into(), input_schema: json!({"type":"object","properties":{}}) },
            McpTool { name: "run_plugin".into(), description: "Run an enabled plugin command with the user's approval. It operates on the active editor buffer, or on the optional workspace file. Edits are undoable and unsaved; do not overwrite them with disk edits.".into(), input_schema: json!({"type":"object","properties":{"plugin":{"type":"string"},"command":{"type":"string"},"arguments":{"type":"object"},"file":{"type":"string"}},"required":["plugin","command"]}) },
        ]);
    }
    if options.sub_agents {
        tools.extend([
            McpTool { name: "list_sub_agents".into(), description: "List enabled specialist sub-agent profiles. Each has an isolated conversation and a bounded step budget.".into(), input_schema: json!({"type":"object","properties":{}}) },
            McpTool { name: "delegate_task".into(), description: "Delegate a focused task to an enabled specialist and wait for its result. Workers receive a scoped brief, inherited instructions/restrictions and an absolute deadline. Nested delegation is depth/budget bounded; wait for results and verify evidence.".into(), input_schema: json!({"type":"object","properties":{"agent":{"type":"string","description":"Exact profile ID from list_sub_agents"},"task":{"type":"string"},"acceptance_criteria":{"type":"string","description":"Concrete evidence/checks required for success; max 8192 bytes"},"constraints":{"type":"string","description":"Scope, file ownership and non-goals; max 8192 bytes; inherited by descendants"},"context":{"type":"string","description":"Relevant parent findings, not entire tool history; max 16 KiB"},"context_files":{"type":"array","maxItems":6,"items":{"type":"string"},"description":"Workspace text paths to snapshot, at most 6; omit to inherit parent references, [] explicitly selects none. Ancestor notes/rules remain. Total brief context max 48 KiB; use small relevant files."}},"required":["agent","task"]}) },
        ]);
    }
    tools.extend([
        McpTool{name:"task_status".into(),description:"Inspect overwatch task lineage, progress, deadlines, waiting phases and terminal results. Workers see only their own subtree.".into(),input_schema:json!({"type":"object","properties":{}})},
        McpTool{name:"report_progress".into(),description:"Report concrete progress, evidence or a blocker. Does not extend any deadline or grant capabilities.".into(),input_schema:json!({"type":"object","properties":{"message":{"type":"string","maxLength":2000}},"required":["message"]})},
        McpTool{name:"cancel_task".into(),description:"Cancel an active descendant task and its entire subtree, including pending approvals. Partial edits are not rolled back. Workers cannot cancel ancestors or siblings.".into(),input_schema:json!({"type":"object","properties":{"task_id":{"type":"integer","minimum":1}},"required":["task_id"]})},
    ]);
    if options.generated_tools {
        tools.extend([
            McpTool{name:"define_function".into(),description:"Create a reusable, task-scoped pure function from validated JSON Schema and 1–12 declarative transformations. Returns a callable function schema. Never executes generated source code, shell, filesystem/network I/O or other tools. Limits: 16 functions/task, 16 KiB definition, 64 KiB input/output.".into(),input_schema:json!({"type":"object","properties":crate::generated_tools::definition_properties(),"required":["name","description","input_schema","steps"],"additionalProperties":false})},
            McpTool{name:"list_functions".into(),description:"List this task's reusable pure function schemas, including copies inherited from its parent.".into(),input_schema:json!({"type":"object","properties":{}})},
            McpTool{name:"call_function".into(),description:"Call a previously defined pure function. Also available under its fn_ name to the built-in agent; MCP agents can always use this wrapper.".into(),input_schema:json!({"type":"object","properties":{"name":{"type":"string"},"arguments":{"type":"object"}},"required":["name","arguments"]})},
        ]);
    }
    tools
}

impl Bridge {
    pub fn start() -> io::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let (tx, requests) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let stop = shutdown.clone();
        let token = token();
        let key = token.clone();
        let connections = Arc::new(std::sync::Mutex::new(Vec::<TcpStream>::new()));
        let clients = connections.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if let Ok(mut clients) = clients.lock() {
                            // A session needs one relay; cap idle client threads.
                            if clients.len() >= 32 {
                                continue;
                            }
                            if let Ok(copy) = stream.try_clone() {
                                clients.push(copy);
                            }
                        }
                        let (tx, key, stop) = (tx.clone(), key.clone(), stop.clone());
                        let registry = clients.clone();
                        let peer = stream.peer_addr().ok();
                        let guard = stream.try_clone().ok();
                        std::thread::spawn(move || {
                            let _ = serve(stream, &key, tx, stop);
                            if let Some(stream) = guard {
                                let _ = stream.shutdown(std::net::Shutdown::Both);
                            }
                            if let Ok(mut clients) = registry.lock() {
                                clients.retain(|s| s.peer_addr().ok() != peer);
                            }
                        });
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(40))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            requests,
            address,
            token,
            shutdown,
            connections,
        })
    }

    pub fn mcp_server(&self) -> io::Result<Value> {
        Ok(
            json!({"name":"termed","command":std::env::current_exe()?.to_string_lossy(),
            "args":["--mcp-bridge",self.address.to_string()],
            "env":[{"name":"TERMED_BRIDGE_TOKEN","value":self.token}]}),
        )
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Ok(clients) = self.connections.lock() {
            for client in clients.iter() {
                let _ = client.shutdown(std::net::Shutdown::Both);
            }
        }
    }
}

fn token() -> String {
    let mut bytes = [0u8; 32];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_ok()
    {
        return bytes.iter().map(|b| format!("{b:02x}")).collect();
    }
    // RandomState uses a per-instance OS-seeded key on supported Rust targets.
    use std::hash::{BuildHasher, Hasher};
    (0..4)
        .map(|_| {
            format!(
                "{:016x}",
                std::collections::hash_map::RandomState::new()
                    .build_hasher()
                    .finish()
            )
        })
        .collect()
}

fn line(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut buf = Vec::new();
    let n = reader.take(1024 * 1024 + 1).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if n > 1024 * 1024 || !buf.ends_with(b"\n") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "MCP message exceeds 1 MB or has no newline",
        ));
    }
    serde_json::from_slice(&buf)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn send(writer: &mut impl Write, v: &Value) -> io::Result<()> {
    writeln!(writer, "{v}")?;
    writer.flush()
}

fn serve(
    mut stream: TcpStream,
    token: &str,
    tx: Sender<Request>,
    stop: Arc<AtomicBool>,
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let Some(auth) = line(&mut reader)? else {
        return Ok(());
    };
    if auth.get("token").and_then(Value::as_str) != Some(token) {
        return Ok(());
    }
    send(&mut stream, &json!({"ok":true}))?;
    // A relay may be idle for a long time. Bound the lifetime and check stop
    // between complete messages; the UI never blocks on a client socket.
    stream.set_read_timeout(Some(Duration::from_secs(600)))?;
    while !stop.load(Ordering::Relaxed) {
        let Some(v) = line(&mut reader)? else { break };
        let Some(id) = v.get("id").cloned() else {
            continue;
        };
        let method = v.get("method").and_then(Value::as_str).unwrap_or("");
        let params = v.get("params").cloned().unwrap_or(json!({}));
        let result = match method {
            "initialize" => Ok(
                json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},
                "serverInfo":{"name":"termed","version":env!("CARGO_PKG_VERSION")}}),
            ),
            "ping" => Ok(json!({})),
            "tools/list" | "tools/call" => {
                let (reply, rx) = mpsc::channel();
                if tx
                    .send(Request {
                        method: method.into(),
                        params,
                        reply,
                    })
                    .is_err()
                {
                    break;
                }
                rx.recv_timeout(Duration::from_secs(600))
                    .unwrap_or_else(|_| Err("Editor disconnected or tool timed out".into()))
            }
            _ => {
                send(
                    &mut stream,
                    &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unsupported MCP method"}}),
                )?;
                continue;
            }
        };
        let result = if method == "tools/call" {
            match result {
                Ok(value) => {
                    json!({"content":[{"type":"text","text":value.as_str().map(String::from).unwrap_or_else(|| value.to_string())}],"isError":false})
                }
                Err(error) => json!({"content":[{"type":"text","text":error}],"isError":true}),
            }
        } else {
            match result {
                Ok(v) => v,
                Err(e) => {
                    send(
                        &mut stream,
                        &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":e}}),
                    )?;
                    continue;
                }
            }
        };
        send(
            &mut stream,
            &json!({"jsonrpc":"2.0","id":id,"result":result}),
        )?;
    }
    Ok(())
}

/// Child-process entry point. stdout contains JSON-RPC only.
pub fn run_stdio(address: &str) -> anyhow::Result<()> {
    let address: SocketAddr = address.parse()?;
    anyhow::ensure!(address.ip().is_loopback(), "MCP bridge must be on loopback");
    let token = std::env::var("TERMED_BRIDGE_TOKEN")
        .map_err(|_| anyhow::anyhow!("Missing MCP bridge token"))?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(620)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    send(&mut stream, &json!({"token":token}))?;
    anyhow::ensure!(
        line(&mut reader)?.and_then(|v| v.get("ok").and_then(Value::as_bool)) == Some(true),
        "MCP authentication failed"
    );
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut out = io::stdout().lock();
    while let Some(v) = line(&mut input)? {
        let request = v.get("id").is_some();
        send(&mut stream, &v)?;
        if request {
            let Some(response) = line(&mut reader)? else {
                break;
            };
            send(&mut out, &response)?;
        }
    }
    Ok(())
}
