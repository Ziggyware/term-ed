//! Plugins: extra commands for termed, written in any language.
//!
//! A plugin is a folder with a manifest and an executable. termed starts the
//! executable as a subprocess and talks **JSON-RPC 2.0**, one message per
//! line, over stdin/stdout — the same shape as the Language Server Protocol
//! and the Agent Client Protocol, so anything that already speaks either is
//! a small adapter away, and a plugin can be written in Python, Node, Go,
//! Rust, a shell script or anything else that can read a line of JSON.
//!
//! # Manifests
//!
//! `plugin.json` is the native format:
//!
//! ```json
//! {
//!   "name": "text-tools",
//!   "version": "1.0.0",
//!   "description": "Sort, trim and format the buffer",
//!   "command": ["python3", "main.py"],
//!   "events": ["startup", "save"],
//!   "commands": [
//!     { "id": "sort", "title": "Text: Sort Lines", "keys": "ctrl+alt+s" }
//!   ]
//! }
//! ```
//!
//! VS Code extension manifests (`package.json` with `contributes.commands`)
//! are also read: the commands show up in the palette and the extension is
//! reported as needing the VS Code runtime, unless the package adds a
//! `"termed": { "command": [...] }` block naming how to run it — that is the
//! bridge for a wrapper script around a JS extension.
//!
//! # Calling back into the editor
//!
//! Plugins send requests to termed:
//!
//! | Method | Params | Result |
//! | --- | --- | --- |
//! | `editor.text` | — | the active buffer |
//! | `editor.path` | — | its path, or `null` |
//! | `editor.selection` | — | the selected text |
//! | `editor.insert` | `{text}` | `null` |
//! | `editor.replace` | `{text}` | `null` |
//! | `editor.open` | `{path, line?}` | `null` |
//! | `project.root` | — | the project folder |
//! | `project.files` | `{limit?}` | relative paths |
//! | `ui.toast` | `{message, kind?}` | `null` |
//!
//! termed sends `event` notifications (`startup`, `open`, `save`) and
//! `command` requests for palette entries and keybindings.
//!
//! # Where plugins live
//!
//! `<project>/.termed/plugins/<name>/`, `~/.config/termed/plugins/<name>/`
//! and `~/.local/share/termed/plugins/<name>/`. The bundled plugins are
//! embedded in the binary and written to `~/.cache/termed/plugins/<name>/`
//! on first run. A plugin is disabled by listing its name in
//! `~/.config/termed/plugins.json` (`{"disabled": ["stats"]}`); the plugin
//! manager in the palette toggles this for you.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};

use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use serde_json::{Value, json};

/// Line length we are willing to read from a plugin (a runaway plugin must
/// not eat all the memory).
const MAX_LINE: u64 = 1 << 20;
/// Most plugins loaded from disk.
const MAX_PLUGINS: usize = 64;

/// One runnable command contributed by a plugin.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandSpec {
    pub id: String,
    pub title: String,
    /// Optional global binding, e.g. `ctrl+alt+s`.
    pub keys: Option<String>,
}

/// Which ecosystem the manifest came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Origin {
    #[default]
    Native,
    /// A VS Code extension manifest.
    VsCode,
}

/// A plugin found on disk.
#[derive(Clone, Debug)]
pub struct Plugin {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub origin: Origin,
    pub dir: PathBuf,
    /// How to start it; empty when the plugin cannot run here.
    pub argv: Vec<String>,
    pub commands: Vec<CommandSpec>,
    pub events: Vec<String>,
    pub enabled: bool,
    /// Why the plugin is unusable, shown in the manager.
    pub problem: Option<String>,
}

impl Plugin {
    pub fn runnable(&self) -> bool {
        !self.argv.is_empty() && self.problem.is_none()
    }

    /// One line for the plugin manager.
    pub fn status(&self) -> String {
        if !self.enabled {
            "disabled".to_string()
        } else if let Some(p) = &self.problem {
            p.clone()
        } else if self.runnable() {
            format!("{} command(s)", self.commands.len())
        } else {
            "no entry point".to_string()
        }
    }
}

// ================================================================ manifests

/// Read whichever manifest a plugin folder has: native first, then VS Code.
pub fn load(dir: &Path) -> Option<Plugin> {
    let native = dir.join("plugin.json");
    if let Ok(text) = std::fs::read_to_string(&native) {
        return parse_manifest(dir, &text, Origin::Native);
    }
    let vscode = dir.join("package.json");
    if let Ok(text) = std::fs::read_to_string(&vscode) {
        return parse_manifest(dir, &text, Origin::VsCode);
    }
    None
}

fn string(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Parse a manifest. Bad JSON, or a manifest without a name, yields `None`.
pub fn parse_manifest(dir: &Path, text: &str, origin: Origin) -> Option<Plugin> {
    let v: Value = serde_json::from_str(text).ok()?;
    let name = v
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| v.get("displayName").and_then(Value::as_str))?
        .to_string();
    let display = v
        .get("displayName")
        .and_then(Value::as_str)
        .unwrap_or(&name)
        .to_string();

    // `command` is an argv array (or a single string, split on whitespace).
    let mut argv: Vec<String> = match v.get("command") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(|s| s.to_string())
            .collect(),
        Some(Value::String(s)) => split_command(s),
        _ => Vec::new(),
    };

    let mut commands = Vec::new();
    if let Some(list) = v.get("commands").and_then(Value::as_array) {
        for c in list {
            let id = c
                .get("id")
                .or_else(|| c.get("command"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            let title = c
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_string();
            commands.push(CommandSpec {
                id,
                title,
                keys: c.get("keys").and_then(Value::as_str).map(str::to_string),
            });
        }
    }
    if commands.is_empty()
        && let Some(list) = v
            .get("contributes")
            .and_then(|c| c.get("commands"))
            .and_then(Value::as_array)
    {
        // VS Code shape: `{command, title}`.
        for c in list {
            let id = c
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            let title = c
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_string();
            commands.push(CommandSpec {
                id,
                title,
                keys: None,
            });
        }
    }

    // A VS Code extension can name a script to run: `"termed": {"command": …}`.
    if argv.is_empty()
        && let Some(t) = v.get("termed").and_then(|t| t.get("command"))
    {
        argv = match t {
            Value::Array(a) => a
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            Value::String(s) => split_command(s),
            _ => Vec::new(),
        };
    }

    let mut events: Vec<String> = v
        .get("events")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if events.is_empty() {
        // VS Code activation events: `onStartupFinished`, `onLanguage:rust`.
        if let Some(list) = v.get("activationEvents").and_then(Value::as_array) {
            let list: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
            if list.iter().any(|e| e.starts_with("onStartup")) {
                events.push("startup".into());
            }
            if list.iter().any(|e| e.starts_with("onLanguage")) {
                events.push("open".into());
            }
            if list.contains(&"*") {
                events.push("startup".into());
                events.push("open".into());
                events.push("save".into());
            }
        }
    }
    events.sort();
    events.dedup();

    let mut problem = None;
    if argv.is_empty() {
        problem = Some(match origin {
            Origin::VsCode => "VS Code extension: add a \"termed\" command to run it".to_string(),
            Origin::Native => "manifest has no \"command\"".to_string(),
        });
    } else if which_program(&argv[0]).is_none() {
        // `python3` is `python` on Windows; everything else must be on PATH.
        if argv[0] == "python3" && which_program("python").is_some() {
            argv[0] = "python".to_string();
        } else {
            problem = Some(format!("{} is not installed", argv[0]));
        }
    }

    Some(Plugin {
        name: display,
        version: string(&v, "version"),
        description: string(&v, "description"),
        author: v
            .get("publisher")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        origin,
        dir: dir.to_path_buf(),
        argv,
        commands,
        events,
        enabled: true,
        problem,
    })
}

/// Look a program up on PATH. Plugin manifests name executables, and a
/// missing one is a manifest problem rather than a crash.
pub fn which_program(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.components().count() > 1 || path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    let dirs = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    for dir in dirs {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        for ext in ["exe", "cmd", "bat"] {
            let candidate = dir.join(format!("{program}.{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn split_command(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if quote == Some('\'') {
            if c == '\'' {
                quote = None;
            } else {
                word.push(c);
            }
        } else if Some(c) == quote {
            quote = None;
        } else if c == '\\'
            && chars.peek().is_some_and(|next| {
                *next == '\\'
                    || *next == '"'
                    || (quote.is_none() && (*next == '\'' || next.is_whitespace()))
            })
        {
            word.push(chars.next().unwrap());
        } else if quote.is_none() && matches!(c, '\'' | '"') {
            quote = Some(c);
            started = true;
        } else if quote.is_none() && c.is_whitespace() {
            if started {
                args.push(std::mem::take(&mut word));
                started = false;
            }
        } else {
            word.push(c);
            started = true;
        }
    }
    if quote.is_some() {
        return Vec::new();
    }
    if started {
        args.push(word);
    }
    args
}

/// Parse a binding like `ctrl+alt+shift+s` or `f5`.
pub fn parse_keys(spec: &str) -> Option<(KeyCode, KeyModifiers)> {
    let mut mods = KeyModifiers::NONE;
    let mut key = None;
    for part in spec.split('+') {
        let p = part.trim().to_ascii_lowercase();
        match p.as_str() {
            "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
            "alt" | "option" => mods |= KeyModifiers::ALT,
            "shift" => mods |= KeyModifiers::SHIFT,
            "" => {}
            _ => {
                key = Some(match p.as_str() {
                    "enter" | "return" => KeyCode::Enter,
                    "esc" | "escape" => KeyCode::Esc,
                    "tab" => KeyCode::Tab,
                    "space" => KeyCode::Char(' '),
                    "up" => KeyCode::Up,
                    "down" => KeyCode::Down,
                    "left" => KeyCode::Left,
                    "right" => KeyCode::Right,
                    "home" => KeyCode::Home,
                    "end" => KeyCode::End,
                    "pgup" | "pageup" => KeyCode::PageUp,
                    "pgdn" | "pagedown" => KeyCode::PageDown,
                    "del" | "delete" => KeyCode::Delete,
                    "backspace" => KeyCode::Backspace,
                    f if f.starts_with('f') && f[1..].parse::<u8>().is_ok() => {
                        KeyCode::F(f[1..].parse().ok()?)
                    }
                    c if c.chars().count() == 1 => KeyCode::Char(c.chars().next()?),
                    _ => return None,
                });
            }
        }
    }
    key.map(|k| (k, mods))
}

// ================================================================ discovery

/// Where termed looks for plugins, most local first.
pub fn search_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![root.join(".termed/plugins")];
    dirs.push(config_home().join("termed/plugins"));
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".local/share/termed/plugins"));
    }
    dirs
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn cache_home() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
}

fn config_file() -> PathBuf {
    config_home().join("termed/plugins.json")
}

/// Names listed in `plugins.json` as disabled.
fn load_disabled(path: &Path) -> HashSet<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashSet::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return HashSet::new();
    };
    v.get("disabled")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Persist the disabled list, keeping the file human-editable.
fn save_disabled(path: &Path, names: &HashSet<String>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut list: Vec<&String> = names.iter().collect();
    list.sort();
    let text = serde_json::to_string_pretty(&json!({ "disabled": list })).unwrap_or_default();
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text + "\n")?;
    std::fs::rename(tmp, path)
}

// ================================================================ bundled

/// A plugin shipped inside the binary: name, file name, contents.
struct Bundled {
    name: &'static str,
    files: &'static [(&'static str, &'static str)],
}

const BUNDLED: &[Bundled] = &[
    Bundled {
        name: "text-tools",
        files: &[
            (
                "plugin.json",
                include_str!("../plugins/text-tools/plugin.json"),
            ),
            ("main.py", include_str!("../plugins/text-tools/main.py")),
        ],
    },
    Bundled {
        name: "todo",
        files: &[
            ("plugin.json", include_str!("../plugins/todo/plugin.json")),
            ("main.py", include_str!("../plugins/todo/main.py")),
        ],
    },
    Bundled {
        name: "stats",
        files: &[
            ("plugin.json", include_str!("../plugins/stats/plugin.json")),
            ("main.py", include_str!("../plugins/stats/main.py")),
        ],
    },
    Bundled {
        name: "git-info",
        files: &[
            (
                "plugin.json",
                include_str!("../plugins/git-info/plugin.json"),
            ),
            ("main.py", include_str!("../plugins/git-info/main.py")),
        ],
    },
];

/// Directory the bundled plugins are written to.
pub fn bundled_dir() -> PathBuf {
    cache_home().join("termed/plugins")
}

/// Write the embedded plugins to the cache directory. Existing files are
/// replaced only when their contents differ, so edits to a bundled plugin
/// are not clobbered on the next start... unless the binary is newer.
pub fn install_bundled() -> std::io::Result<PathBuf> {
    let root = bundled_dir();
    for b in BUNDLED {
        let dir = root.join(b.name);
        std::fs::create_dir_all(&dir)?;
        for (file, text) in b.files {
            let path = dir.join(file);
            let same = std::fs::read_to_string(&path).is_ok_and(|t| t == *text);
            if !same {
                std::fs::write(&path, text)?;
            }
        }
    }
    Ok(root)
}

// ================================================================ process

struct Worker {
    child: Child,
    /// Lines waiting to be written to the plugin's stdin. A thread owns the
    /// pipe so a plugin that stops reading can never block the editor.
    out: Sender<String>,
}

/// A message from a plugin process.
#[derive(Clone, Debug)]
pub enum Msg {
    /// The plugin is calling into the editor.
    Request {
        plugin: usize,
        id: Option<u64>,
        method: String,
        params: Value,
    },
    /// stderr or an unparsable line; useful in the log/toast.
    Junk { plugin: usize, line: String },
    /// A command completed (including its JSON-RPC error, if any).
    Response {
        plugin: usize,
        id: u64,
        result: Result<Value, String>,
    },
    /// The process ended.
    Exited { plugin: usize, code: Option<i32> },
}

/// The plugin host: discovery, processes and the JSON-RPC plumbing.
pub struct Host {
    pub plugins: Vec<Plugin>,
    pub root: PathBuf,
    /// Persist to the same file this host loaded, even if the environment changes.
    config_path: PathBuf,
    procs: HashMap<usize, Worker>,
    rx: Receiver<(usize, Value)>,
    tx: Sender<(usize, Value)>,
    /// Requests termed sent that a plugin has not answered yet.
    pending: HashMap<u64, (usize, String)>,
    next_id: u64,
    /// Failed starts (kept out of `problem` so the manifest stays clean).
    pub start_errors: HashMap<usize, String>,
}

impl Host {
    pub fn new(root: PathBuf) -> Self {
        let (tx, rx) = channel();
        let mut host = Self {
            plugins: Vec::new(),
            root,
            config_path: config_file(),
            procs: HashMap::new(),
            rx,
            tx,
            pending: HashMap::new(),
            next_id: 1,
            start_errors: HashMap::new(),
        };
        host.reload();
        host
    }

    /// Find plugins again; running processes are stopped first.
    pub fn reload(&mut self) {
        for (_, mut p) in self.procs.drain() {
            crate::process_guard::terminate(&mut p.child);
        }
        self.plugins.clear();
        self.pending.clear();
        while self.rx.try_recv().is_ok() {}
        self.start_errors.clear();
        let disabled = load_disabled(&self.config_path);
        let mut seen = HashSet::new();
        let mut dirs = search_dirs(&self.root);
        if let Ok(bundled) = install_bundled() {
            dirs.push(bundled);
        }
        'outer: for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut entries: Vec<PathBuf> =
                entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
            entries.sort();
            for path in entries {
                if !path.is_dir() {
                    continue;
                }
                let Some(mut plugin) = load(&path) else {
                    continue;
                };
                // A project-local plugin with the same name wins.
                if !seen.insert(plugin.name.clone()) {
                    continue;
                }
                plugin.enabled =
                    !disabled.contains(&plugin.name) && !disabled.contains(&slug(&plugin.name));
                self.plugins.push(plugin);
                if self.plugins.len() >= MAX_PLUGINS {
                    break 'outer;
                }
            }
        }
    }

    /// Enabled, runnable plugins in a stable order.
    pub fn active(&self) -> impl Iterator<Item = (usize, &Plugin)> {
        self.plugins
            .iter()
            .enumerate()
            .filter(|(_, p)| p.enabled && p.runnable())
    }

    /// Every command: `(plugin index, command index)` with its title.
    pub fn commands(&self) -> Vec<(usize, usize, String)> {
        let mut out = Vec::new();
        for (pi, p) in self.active() {
            for (ci, c) in p.commands.iter().enumerate() {
                out.push((pi, ci, format!("{}: {}", p.name, c.title)));
            }
        }
        out
    }

    /// The command a key press should run, if a plugin claimed it.
    pub fn key_action(&self, code: KeyCode, mods: KeyModifiers) -> Option<(usize, usize)> {
        for (pi, p) in self.active() {
            for (ci, c) in p.commands.iter().enumerate() {
                let Some(spec) = &c.keys else { continue };
                if parse_keys(spec) == Some((code, mods)) {
                    return Some((pi, ci));
                }
            }
        }
        None
    }

    /// Start a plugin if it is not running, sending it `startup` when it
    /// asked for that event.
    pub fn start(&mut self, pi: usize) {
        if self.procs.contains_key(&pi) {
            return;
        }
        let Some(plugin) = self.plugins.get(pi) else {
            return;
        };
        if !plugin.enabled || !plugin.runnable() {
            return;
        }
        let argv = plugin.argv.clone();
        let dir = plugin.dir.clone();
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .current_dir(&dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::process_guard::configure(&mut cmd);
        match cmd.spawn() {
            Ok(mut child) => {
                let Some(stdin) = child.stdin.take() else {
                    self.start_errors.insert(pi, "no stdin".into());
                    return;
                };
                if let Some(stdout) = child.stdout.take() {
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        let mut reader = BufReader::new(stdout);
                        let mut line = String::new();
                        loop {
                            line.clear();
                            match reader.read_line(&mut line) {
                                Ok(0) | Err(_) => break,
                                Ok(_) => {
                                    if line.len() as u64 > MAX_LINE {
                                        continue;
                                    }
                                    if let Ok(v) = serde_json::from_str::<Value>(line.trim())
                                        && tx.send((pi, v)).is_err()
                                    {
                                        break;
                                    }
                                }
                            }
                        }
                    });
                }
                if let Some(stderr) = child.stderr.take() {
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        let mut reader = BufReader::new(stderr);
                        let mut line = String::new();
                        while reader.read_line(&mut line).unwrap_or(0) > 0 {
                            if tx.send((pi, json!({"stderr": line.trim_end()}))).is_err() {
                                break;
                            }
                            line.clear();
                        }
                    });
                }
                let (out, lines) = channel::<String>();
                std::thread::spawn(move || {
                    let mut stdin = stdin;
                    for line in lines {
                        if stdin.write_all(line.as_bytes()).is_err() {
                            break;
                        }
                        let _ = stdin.flush();
                    }
                });
                self.procs.insert(pi, Worker { child, out });
                self.start_errors.remove(&pi);
                let wants_startup = self
                    .plugins
                    .get(pi)
                    .is_some_and(|p| p.events.iter().any(|e| e == "startup"));
                if wants_startup {
                    self.notify(
                        pi,
                        "startup",
                        json!({ "root": self.root.to_string_lossy() }),
                    );
                }
            }
            Err(e) => {
                self.start_errors.insert(pi, e.to_string());
            }
        }
    }

    /// Send a notification (`event`) to one plugin.
    pub fn notify(&mut self, pi: usize, name: &str, params: Value) {
        self.write(
            pi,
            json!({ "jsonrpc": "2.0", "method": "event",
                    "params": { "name": name, "params": params } }),
        );
    }

    /// Send an event to every plugin that subscribed to it.
    pub fn broadcast(&mut self, name: &str, params: Value) {
        let targets: Vec<usize> = self
            .plugins
            .iter()
            .enumerate()
            .filter(|(_, p)| p.enabled && p.runnable() && p.events.iter().any(|e| e == name))
            .map(|(i, _)| i)
            .collect();
        for pi in targets {
            self.start(pi);
            self.notify(pi, name, params.clone());
        }
    }

    /// Run a command: start the plugin if needed and send the request.
    pub fn run(&mut self, pi: usize, ci: usize, file: Option<&Path>) {
        let _ = self.run_request(pi, ci, file, json!({}));
    }

    pub fn busy_plugin(&self, pi: usize) -> bool {
        self.pending.values().any(|(owner, _)| *owner == pi)
    }

    /// Run a validated command and return its request ID so AI callers can
    /// await the actual response, not mistake process startup for success.
    pub fn run_request(
        &mut self,
        pi: usize,
        ci: usize,
        file: Option<&Path>,
        arguments: Value,
    ) -> Result<u64, String> {
        let plugin = self.plugins.get(pi).ok_or("Plugin not found")?;
        if !plugin.enabled || !plugin.runnable() {
            return Err("Plugin is disabled or cannot run".into());
        }
        if self.busy_plugin(pi) {
            return Err("Plugin already has a command in progress".into());
        }
        let id = plugin
            .commands
            .get(ci)
            .ok_or("Plugin command not found")?
            .id
            .clone();
        self.start(pi);
        if let Some(error) = self.start_errors.get(&pi) {
            return Err(format!("Cannot start plugin: {error}"));
        }
        if !self.running_plugin(pi) {
            return Err("Plugin did not start".into());
        }
        let rid = self.next_id;
        self.next_id += 1;
        self.pending.insert(rid, (pi, id.clone()));
        self.write(
            pi,
            json!({"jsonrpc":"2.0","id":rid,"method":"command",
            "params":{"id":id,"root":self.root.to_string_lossy(),
                "file":file.map(|f| f.to_string_lossy().into_owned()),"arguments":arguments}}),
        );
        Ok(rid)
    }

    /// Answer a plugin's request.
    pub fn reply(&mut self, pi: usize, id: u64, result: Value) {
        self.write(pi, json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    fn write(&mut self, pi: usize, msg: Value) {
        let Some(proc) = self.procs.get(&pi) else {
            return;
        };
        let mut text = msg.to_string();
        text.push('\n');
        // The writer thread owns the pipe; if it has gone away the message is
        // dropped, and `pump` reports the exit.
        let _ = proc.out.send(text);
    }

    /// Take everything the plugins have said since the last call.
    pub fn pump(&mut self) -> Vec<Msg> {
        let mut out = Vec::new();
        while let Ok((pi, v)) = self.rx.try_recv() {
            if let Some(text) = v.get("stderr").and_then(Value::as_str) {
                out.push(Msg::Junk {
                    plugin: pi,
                    line: text.to_string(),
                });
                continue;
            }
            let id = v.get("id").and_then(Value::as_u64);
            if let Some(method) = v.get("method").and_then(Value::as_str) {
                out.push(Msg::Request {
                    plugin: pi,
                    id,
                    method: method.to_string(),
                    params: v.get("params").cloned().unwrap_or(Value::Null),
                });
            } else if let Some(id) = id {
                if self.pending.get(&id).is_some_and(|(owner, _)| *owner == pi) {
                    self.pending.remove(&id);
                    let result = if let Some(error) = v.get("error") {
                        Err(error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("Plugin command failed")
                            .to_string())
                    } else {
                        Ok(v.get("result").cloned().unwrap_or(Value::Null))
                    };
                    out.push(Msg::Response {
                        plugin: pi,
                        id,
                        result,
                    });
                }
            }
        }
        // Reap dead processes so the manager can show them as stopped.
        let mut dead = Vec::new();
        for (pi, proc) in self.procs.iter_mut() {
            if let Ok(Some(status)) = proc.child.try_wait() {
                dead.push((*pi, status.code()));
            }
        }
        for (pi, code) in dead {
            self.procs.remove(&pi);
            // Commands it never answered are not going to be answered.
            self.pending.retain(|_, (owner, _)| *owner != pi);
            out.push(Msg::Exited { plugin: pi, code });
        }
        out
    }

    /// Stop a running plugin.
    pub fn stop(&mut self, pi: usize) {
        self.pending.retain(|_, (owner, _)| *owner != pi);
        if let Some(mut p) = self.procs.remove(&pi) {
            crate::process_guard::terminate(&mut p.child);
        }
    }

    /// Enable/disable a plugin and remember the choice.
    pub fn set_enabled(&mut self, pi: usize, enabled: bool) {
        let _ = self.try_set_enabled(pi, enabled);
    }

    pub fn try_set_enabled(&mut self, pi: usize, enabled: bool) -> std::io::Result<()> {
        let Some(p) = self.plugins.get(pi) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Plugin not found",
            ));
        };
        let mut disabled = load_disabled(&self.config_path);
        let key = slug(&p.name);
        if enabled {
            disabled.remove(&key);
            disabled.remove(&p.name);
        } else {
            disabled.insert(key);
        }
        save_disabled(&self.config_path, &disabled)?;
        self.plugins[pi].enabled = enabled;
        if !enabled {
            self.stop(pi);
        }
        Ok(())
    }

    /// `true` while any plugin process is running (keeps the UI ticking).
    pub fn running(&self) -> bool {
        !self.procs.is_empty()
    }

    pub fn running_plugin(&self, pi: usize) -> bool {
        self.procs.contains_key(&pi)
    }

    /// Give plugins a moment to start before the first frame.
    pub fn warm_up(&mut self) {
        let eager: Vec<usize> = self
            .plugins
            .iter()
            .enumerate()
            .filter(|(_, p)| p.enabled && p.runnable() && p.events.iter().any(|e| e == "startup"))
            .map(|(i, _)| i)
            .collect();
        for pi in eager {
            self.start(pi);
        }
        // Nothing to wait for: the reader threads deliver replies as they
        // arrive, and the UI keeps ticking while plugins are running.
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        // Do not leave plugin processes behind when the editor quits.
        for (_, mut p) in self.procs.drain() {
            crate::process_guard::terminate(&mut p.child);
        }
    }
}

/// Config keys use the manifest name, lower-cased and dashed.
fn slug(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

// ================================================================ tests

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn native(name: &str, extra: &str) -> String {
        format!(
            r#"{{"name":"{name}","version":"1.0.0","description":"d","command":["sh","-c","cat"],"commands":[{{"id":"a","title":"Do A","keys":"ctrl+alt+a"}}]{extra}}}"#
        )
    }

    #[test]
    fn native_manifest_is_read() {
        let p = parse_manifest(Path::new("/p/one"), &native("one", ""), Origin::Native).unwrap();
        assert_eq!(p.name, "one");
        assert_eq!(p.argv, vec!["sh", "-c", "cat"]);
        assert_eq!(p.commands[0].id, "a");
        assert_eq!(p.commands[0].title, "Do A");
        assert_eq!(p.commands[0].keys.as_deref(), Some("ctrl+alt+a"));
        assert!(p.enabled);
        assert!(
            p.problem.is_none()
                || p.problem
                    .as_deref()
                    .is_some_and(|m| m.contains("installed"))
        );
    }

    #[test]
    fn vscode_package_is_understood() {
        let text = r#"{
            "name": "vscode-thing",
            "displayName": "Thing",
            "publisher": "acme",
            "engines": {"vscode": "^1.80.0"},
            "activationEvents": ["onStartupFinished", "onLanguage:rust"],
            "contributes": {"commands": [
                {"command": "thing.run", "title": "Thing: Run"}
            ]}
        }"#;
        let p = parse_manifest(Path::new("/p/thing"), text, Origin::VsCode).unwrap();
        assert_eq!(p.name, "Thing");
        assert_eq!(p.author, "acme");
        assert_eq!(p.commands[0].id, "thing.run");
        assert_eq!(p.commands[0].title, "Thing: Run");
        assert!(p.events.contains(&"startup".to_string()));
        assert!(p.events.contains(&"open".to_string()));
        // No runtime: listed, but not runnable, with a clear reason.
        assert!(!p.runnable());
        assert!(p.problem.unwrap().contains("VS Code"));

        // The bridge makes it runnable.
        let text = text.replace(
            "\"contributes\"",
            "\"termed\": {\"command\": [\"sh\", \"-c\", \"true\"]}, \"contributes\"",
        );
        let p = parse_manifest(Path::new("/p/thing"), &text, Origin::VsCode).unwrap();
        assert!(p.runnable());
        assert_eq!(p.argv[0], "sh");
    }

    #[test]
    fn bad_manifests_are_ignored() {
        assert!(parse_manifest(Path::new("/p"), "{not json", Origin::Native).is_none());
        assert!(parse_manifest(Path::new("/p"), r#"{"version":"1"}"#, Origin::Native).is_none());
        // A command that is not installed is reported, not dropped.
        let text = r#"{"name":"x","command":["definitely-not-a-real-binary"]}"#;
        let p = parse_manifest(Path::new("/p/x"), text, Origin::Native).unwrap();
        assert!(!p.runnable());
        assert!(p.problem.unwrap().contains("not installed"));
    }

    #[test]
    fn key_specs_parse() {
        assert_eq!(
            parse_keys("ctrl+alt+s"),
            Some((
                KeyCode::Char('s'),
                KeyModifiers::CONTROL | KeyModifiers::ALT
            ))
        );
        assert_eq!(parse_keys("F5"), Some((KeyCode::F(5), KeyModifiers::NONE)));
        assert_eq!(
            parse_keys("shift+ctrl+pageup"),
            Some((KeyCode::PageUp, KeyModifiers::SHIFT | KeyModifiers::CONTROL))
        );
        assert_eq!(parse_keys("hyper+x"), None);
        assert_eq!(parse_keys(""), None);
    }

    #[test]
    fn bundled_plugins_are_embedded_and_written() {
        if crate::test_support::isolate_unit(
            module_path!(),
            "bundled_plugins_are_embedded_and_written",
        ) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("termed-bundled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: single-threaded test setting an env var for this process.
        unsafe { std::env::set_var("XDG_CACHE_HOME", &dir) };
        let root = install_bundled().unwrap();
        assert!(root.starts_with(&dir));
        for b in BUNDLED {
            let manifest = std::fs::read_to_string(root.join(b.name).join("plugin.json")).unwrap();
            let p = parse_manifest(&root.join(b.name), &manifest, Origin::Native).unwrap();
            assert!(!p.commands.is_empty(), "{} has no commands", b.name);
            assert!(p.events.len() <= 3);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plugin_runs_and_talks_json_rpc() {
        if crate::test_support::isolate_unit(module_path!(), "plugin_runs_and_talks_json_rpc") {
            return;
        }
        let root = std::env::temp_dir().join(format!("termed-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join(".termed/plugins/echo");
        std::fs::create_dir_all(&dir).unwrap();
        // A plugin that echoes commands back through the editor API.
        std::fs::write(
            dir.join("main.py"),
            r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    if msg.get("method") == "command":
        print(json.dumps({"jsonrpc": "2.0", "id": msg["id"],
                          "result": {"ok": True}}), flush=True)
        print(json.dumps({"jsonrpc": "2.0", "method": "ui.toast",
                          "params": {"message": "hi " + msg["params"]["id"]}}), flush=True)
    elif msg.get("method") == "event":
        print(json.dumps({"jsonrpc": "2.0", "method": "project.root", "id": 99,
                          "params": {}}), flush=True)
"#,
        )
        .unwrap();
        let manifest = r#"{"name":"echo","command":["python3","main.py"],
            "events":["startup"],"commands":[{"id":"do","title":"Echo: Do"}]}"#;
        std::fs::write(dir.join("plugin.json"), manifest).unwrap();

        let mut host = Host::new(root.clone());
        host.plugins.retain(|p| p.dir == dir);
        assert_eq!(host.plugins.len(), 1, "{:?}", host.plugins);
        assert!(host.plugins[0].runnable());
        assert_eq!(host.commands().len(), 1);

        host.start(0);
        std::thread::sleep(Duration::from_millis(200));
        host.run(0, 0, None);
        std::thread::sleep(Duration::from_millis(300));

        let mut methods = Vec::new();
        for msg in host.pump() {
            match msg {
                Msg::Request {
                    method, params, id, ..
                } => {
                    if method == "ui.toast" {
                        assert_eq!(params["message"], "hi do");
                    }
                    methods.push((method, id));
                }
                Msg::Response { result, .. } => {
                    assert_eq!(result.expect("command succeeded")["ok"], true);
                }
                Msg::Exited { .. } => panic!("plugin exited"),
                Msg::Junk { line, .. } => panic!("junk: {line}"),
            }
        }
        assert!(methods.iter().any(|(m, _)| m == "ui.toast"), "{methods:?}");
        assert!(
            methods
                .iter()
                .any(|(m, id)| m == "project.root" && *id == Some(99)),
            "{methods:?}"
        );
        host.stop(0);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Drive a bundled plugin the way the editor does and check that it
    /// really edits the buffer. Runs the files from the repo, not the cache.
    #[test]
    fn text_tools_sorts_the_buffer() {
        if crate::test_support::isolate_unit(module_path!(), "text_tools_sorts_the_buffer") {
            return;
        }
        let dir = Path::new("plugins/text-tools");
        if !dir.is_dir() {
            return; // not running from the crate root
        }
        let mut host = Host::new(std::env::temp_dir());
        let pi = host
            .plugins
            .iter()
            .position(|p| p.name == "text-tools")
            .unwrap_or_else(|| {
                host.plugins.insert(0, load(dir).unwrap());
                0
            });
        host.plugins[pi] = load(dir).unwrap();
        assert!(
            host.plugins[pi].runnable(),
            "{:?}",
            host.plugins[pi].problem
        );
        host.start(pi);
        host.run(pi, 0, None); // "sort"
        std::thread::sleep(Duration::from_millis(150));

        let mut replaced = None;
        for _ in 0..20 {
            for msg in host.pump() {
                if let Msg::Request {
                    id, method, params, ..
                } = msg
                {
                    match method.as_str() {
                        "editor.text" => host.reply(pi, id.unwrap(), json!("b\na\nc")),
                        "editor.replace" => {
                            replaced = params
                                .get("text")
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            host.reply(pi, id.unwrap(), Value::Null);
                        }
                        "ui.toast" => host.reply(pi, id.unwrap(), Value::Null),
                        _ => {}
                    }
                }
            }
            if replaced.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        host.stop(pi);
        assert_eq!(replaced.as_deref(), Some("a\nb\nc"));
    }
}
