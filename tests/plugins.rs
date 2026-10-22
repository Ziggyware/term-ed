//! Plugins: discovery, the JSON-RPC host, VS Code manifests and the
//! bundled plugins, driven through a real `App` and real subprocesses.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use termed::app::{App, PaletteMode};
use termed::plugin::{self, Host, Origin};

/// The environment is process-wide, and two tests point it at temp folders,
/// so the tests in this file take turns.
static ENV: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    ENV.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run `f` with `XDG_CONFIG_HOME` pointed at `dir`, restoring it after.
fn with_config_home<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let old = std::env::var_os("XDG_CONFIG_HOME");
    // SAFETY: tests in this file are serialized by `lock()`.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", dir) };
    let out = f();
    match old {
        Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
    }
    out
}

fn with_cache_home<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let old = std::env::var_os("XDG_CACHE_HOME");
    // SAFETY: as above.
    unsafe { std::env::set_var("XDG_CACHE_HOME", dir) };
    let out = f();
    match old {
        Some(v) => unsafe { std::env::set_var("XDG_CACHE_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_CACHE_HOME") },
    }
    out
}

fn project(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("termed-plugins-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n// TODO: ship it\n").unwrap();
    std::fs::write(root.join("notes.txt"), "alpha\nbeta\n").unwrap();
    root
}

/// A local plugin that upper-cases the buffer through the editor API.
fn write_upper_plugin(root: &Path) -> PathBuf {
    let dir = root.join(".termed/plugins/upper");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("main.py"),
        r#"
import json, sys

def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()

def ask(rid, method, params=None):
    send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params or {}})

rid = 1000
for line in sys.stdin:
    msg = json.loads(line)
    if msg.get("method") != "command":
        continue
    rid += 1
    ask(rid, "editor.text")
    text = ""
    while True:
        reply = json.loads(sys.stdin.readline())
        if reply.get("id") == rid:
            text = reply.get("result") or ""
            break
    rid += 1
    ask(rid, "editor.replace", {"text": text.upper()})
    rid += 1
    ask(rid, "ui.toast", {"message": "upper: done", "kind": "success"})
    send({"jsonrpc": "2.0", "id": msg["id"], "result": {"ok": True}})
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("plugin.json"),
        r#"{
            "name": "upper",
            "version": "1.0.0",
            "description": "UPPER CASE everything",
            "command": ["python3", "main.py"],
            "commands": [
                {"id": "go", "title": "Upper: Everything", "keys": "ctrl+alt+u"},
                {"id": "half", "title": "Upper: Half"}
            ]
        }"#,
    )
    .unwrap();
    dir
}

fn pump_until(app: &mut App, done: impl Fn(&App) -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        app.pump_plugins();
        if done(app) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn a_local_plugin_is_found_and_listed() {
    let _guard = lock();
    let root = project("discover");
    write_upper_plugin(&root);
    let host = Host::new(root.clone());
    let upper = host
        .plugins
        .iter()
        .find(|p| p.name == "upper")
        .expect("the .termed/plugins folder is searched");
    assert_eq!(upper.version, "1.0.0");
    assert_eq!(upper.origin, Origin::Native);
    assert_eq!(upper.commands.len(), 2);
    assert_eq!(upper.commands[0].title, "Upper: Everything");
    assert_eq!(upper.commands[0].keys.as_deref(), Some("ctrl+alt+u"));
    if python() {
        assert!(upper.runnable(), "{:?}", upper.problem);
    }
    // Names are unique even though several folders can hold plugins.
    let mut names: Vec<&str> = host.plugins.iter().map(|p| p.name.as_str()).collect();
    let total = names.len();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), total);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn bundled_plugins_ship_with_the_binary() {
    let _guard = lock();
    if !python() {
        return;
    }
    let cache = std::env::temp_dir().join(format!("termed-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    let root = project("bundled");
    let host = with_cache_home(&cache, || Host::new(root.clone()));
    for name in ["text-tools", "todo", "stats", "git-info"] {
        let p = host
            .plugins
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("{name} is bundled: {:?}", names(&host)));
        assert!(p.runnable(), "{name}: {:?}", p.problem);
        assert!(!p.commands.is_empty(), "{name} has no commands");
    }
    let cmds = host.commands();
    assert!(cmds.len() >= 12, "{} commands", cmds.len());
    // Titles are prefixed with the plugin so the palette reads well.
    assert!(
        cmds.iter()
            .any(|(_, _, t)| t == "text-tools: Sort Lines (A-Z)")
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&cache);
}

fn names(host: &Host) -> Vec<String> {
    host.plugins.iter().map(|p| p.name.clone()).collect()
}

#[test]
fn a_plugin_command_edits_the_buffer_and_toasts() {
    let _guard = lock();
    if !python() {
        return;
    }
    let root = project("run");
    write_upper_plugin(&root);
    let mut app = App::new(
        root.clone(),
        vec![root.join("notes.txt")],
        "Visual Studio Dark",
        false,
    );
    let pi = app
        .plugins
        .plugins
        .iter()
        .position(|p| p.name == "upper")
        .expect("upper discovered");
    app.run_plugin(pi, 0);
    assert!(
        pump_until(&mut app, |app| app.editor().is_some_and(|e| e
            .doc
            .rope
            .to_string()
            .contains("ALPHA"))),
        "buffer was not replaced; toasts: {:?}",
        app.toasts.iter().map(|t| &t.text).collect::<Vec<_>>()
    );
    assert_eq!(
        app.editor().unwrap().doc.rope.to_string(),
        "ALPHA\nBETA\n",
        "the plugin replaced the whole buffer"
    );
    assert!(
        pump_until(&mut app, |app| app
            .toasts
            .iter()
            .any(|t| t.text.contains("upper: done"))),
        "no toast from the plugin"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_plugin_keybinding_runs_without_the_palette() {
    let _guard = lock();
    if !python() {
        return;
    }
    let root = project("keys");
    write_upper_plugin(&root);
    let mut app = App::new(
        root.clone(),
        vec![root.join("notes.txt")],
        "Visual Studio Dark",
        false,
    );
    // Ctrl+Alt+U is claimed by the plugin's manifest.
    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Char('u'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    )));
    assert!(
        pump_until(&mut app, |app| app.editor().is_some_and(|e| e
            .doc
            .rope
            .to_string()
            .contains("ALPHA"))),
        "the keybinding did not run the plugin"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_vscode_extension_is_listed_with_the_reason_it_cannot_run() {
    let _guard = lock();
    let root = project("vscode");
    let dir = root.join(".termed/plugins/vscode-prettier");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("package.json"),
        r#"{
            "name": "prettier-vscode",
            "displayName": "Prettier",
            "publisher": "esbenp",
            "version": "10.0.0",
            "activationEvents": ["onStartupFinished"],
            "contributes": {"commands": [
                {"command": "prettier.format", "title": "Format Document"}
            ]}
        }"#,
    )
    .unwrap();
    let host = Host::new(root.clone());
    let ext = host
        .plugins
        .iter()
        .find(|p| p.name == "Prettier")
        .expect("VS Code manifests are read");
    assert_eq!(ext.origin, Origin::VsCode);
    assert_eq!(ext.author, "esbenp");
    assert_eq!(ext.commands[0].id, "prettier.format");
    assert!(ext.events.contains(&"startup".to_string()));
    assert!(!ext.runnable());
    assert!(ext.status().contains("VS Code"), "{}", ext.status());
    // It is not offered as a command, since it cannot run here.
    assert!(
        !host
            .commands()
            .iter()
            .any(|(_, _, t)| t.contains("Prettier"))
    );

    // Adding a bridge command makes it usable.
    std::fs::write(
        dir.join("package.json"),
        r#"{
            "name": "prettier-vscode",
            "displayName": "Prettier",
            "termed": {"command": ["sh", "-c", "true"]},
            "contributes": {"commands": [
                {"command": "prettier.format", "title": "Format Document"}
            ]}
        }"#,
    )
    .unwrap();
    let host = Host::new(root.clone());
    let ext = host.plugins.iter().find(|p| p.name == "Prettier").unwrap();
    assert!(ext.runnable(), "{:?}", ext.problem);
    assert!(
        host.commands()
            .iter()
            .any(|(_, _, t)| t.contains("Prettier"))
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn disabled_plugins_are_remembered() {
    let _guard = lock();
    let root = project("toggle");
    write_upper_plugin(&root);
    let config = root.join("termed-config");
    let mut host = with_config_home(&config, || Host::new(root.clone()));
    let pi = host.plugins.iter().position(|p| p.name == "upper").unwrap();
    host.set_enabled(pi, false);
    assert!(!host.plugins[pi].enabled);
    assert!(
        !host.commands().iter().any(|(_, _, t)| t.contains("upper")),
        "a disabled plugin contributes no commands"
    );
    let saved = std::fs::read_to_string(config.join("termed/plugins.json")).unwrap();
    assert!(saved.contains("upper"), "{saved}");
    // A fresh host reads it back.
    let mut again = with_config_home(&config, || Host::new(root.clone()));
    let pi = again
        .plugins
        .iter()
        .position(|p| p.name == "upper")
        .unwrap();
    assert!(!again.plugins[pi].enabled, "the disabled list is persisted");
    with_config_home(&config, || again.set_enabled(pi, true));
    let again = with_config_home(&config, || Host::new(root.clone()));
    let pi = again
        .plugins
        .iter()
        .position(|p| p.name == "upper")
        .unwrap();
    assert!(again.plugins[pi].enabled);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&config);
}

#[test]
fn the_host_api_answers_the_documented_methods() {
    let _guard = lock();
    let root = project("api");
    let app = App::new(
        root.clone(),
        vec![root.join("notes.txt")],
        "Visual Studio Dark",
        false,
    );
    // The bundled installer runs on startup, so the manager has something
    // to show even in a brand new project.
    let dir = plugin::install_bundled().expect("bundled plugins are installed");
    assert!(dir.join("text-tools/plugin.json").exists());
    assert!(dir.join("todo/main.py").exists());
    let text = app.editor().unwrap().doc.rope.to_string();
    assert!(text.contains("alpha"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_keyboard_bindings_do_not_collide() {
    let _guard = lock();
    let root = project("keys-mgr");
    let mut app = App::new(
        root.clone(),
        vec![root.join("notes.txt")],
        "Visual Studio Dark",
        false,
    );
    // Ctrl+Alt+P opens the manager...
    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Char('p'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    )));
    assert_eq!(
        app.palette.as_ref().map(|p| p.mode),
        Some(PaletteMode::Plugins)
    );
    // ...and the quick-open boxes keep their own chords.
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Char('p'),
        KeyModifiers::CONTROL,
    )));
    assert_eq!(
        app.palette.as_ref().map(|p| p.mode),
        Some(PaletteMode::Files)
    );
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Char('p'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    )));
    assert_eq!(
        app.palette.as_ref().map(|p| p.mode),
        Some(PaletteMode::Commands)
    );
    // The manager lists the bundled plugins and a command palette entry for
    // each of their commands.
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    app.run(termed::commands::Command::Plugins);
    let p = app.palette.as_ref().unwrap();
    assert!(
        p.items.iter().any(|i| i.contains("Reload")),
        "{:?}",
        p.items
    );
    assert!(
        p.items.iter().any(|i| i.contains("text-tools")),
        "{:?}",
        p.items
    );
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    app.run(termed::commands::Command::CommandPalette);
    let p = app.palette.as_ref().unwrap();
    // Plugin commands follow the built-in ones, and the palette remembers
    // which plugin command each of them is.
    assert_eq!(
        p.plugin_cmds.len(),
        p.items.len() - termed::commands::COMMANDS.len()
    );
    if python() {
        assert!(
            p.items.iter().any(|i| i == "text-tools: Sort Lines (A-Z)"),
            "{:?}",
            p.items
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}
