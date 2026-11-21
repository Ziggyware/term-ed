//! A provider that answers HTTP 503 must not end the turn: the built-in agent
//! switches to the on-device ONNX server and finishes the conversation there.
//!
//! The fixture gateway answers 503 to every chat completion while advertising
//! a model, and the local server runs the real `local_inference/onnx_server.py`
//! against a tiny deterministic model built by `tests/fixtures/build_tiny_onnx.py`
//! (embedding + linear head, no download), so this exercises the same
//! OpenAI-compatible surface termed talks to in production.
//!
//! Requirement: `python3` with onnxruntime, tokenizers, onnx and numpy
//! (`pip install -r local_inference/requirements.txt`). The test skips when
//! they are missing, so it never fails on a machine without the optional
//! local-inference dependencies.

mod common;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use termed::agent::Entry;
use termed::app::App;
use termed::commands::Command as Cmd;
use termed::config::AgentConfig;

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn python() -> String {
    std::env::var("TERMED_PYTHON").unwrap_or_else(|_| "python3".into())
}

fn have_python() -> bool {
    Command::new(python())
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn have_onnx_runtime() -> bool {
    Command::new(python())
        .args(["-c", "import onnxruntime, tokenizers, numpy"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The committed tiny model, rebuilt only if the checkout lacks it (which
/// needs the extra `onnx` package, see local_inference/requirements-dev.txt).
fn fixture_model() -> bool {
    let model = manifest().join("tests/fixtures/tiny_onnx_model");
    if model.join("model.onnx").is_file() {
        return true;
    }
    Command::new(python())
        .arg(manifest().join("tests/fixtures/build_tiny_onnx.py"))
        .arg(&model)
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Start a script that prints its listening port on the first stdout line.
fn spawn(script: &Path, args: &[&str]) -> Option<(Child, String)> {
    let mut child = Command::new(python())
        .arg(script)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut port = String::new();
    BufReader::new(child.stdout.take()?).read_line(&mut port).ok()?;
    let port = port.trim().to_string();
    (!port.is_empty()).then_some((child, port))
}

/// `/healthz` answers 200 once the model is loaded.
fn healthy(port: &str) -> bool {
    let code = format!(
        "import urllib.request,sys\n\
         try: r=urllib.request.urlopen('http://127.0.0.1:{port}/healthz',timeout=2)\n\
         except Exception: sys.exit(1)\n\
         sys.exit(0 if r.status==200 else 1)\n"
    );
    Command::new(python())
        .args(["-c", &code])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Wait until `/healthz` stops answering (server shut down).
fn wait_down(port: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if !healthy(port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

fn wait_healthy(port: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if healthy(port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// Ask any server on `port` to shut down. termed may have started its own
/// (adopted) server on that port, so this must not kill by process handle.
fn shutdown(port: &str) {
    let code = format!(
        "import urllib.request\n\
         urllib.request.urlopen(urllib.request.Request(\
         'http://127.0.0.1:{port}/admin/shutdown', method='POST'), timeout=2)"
    );
    let _ = Command::new(python())
        .args(["-c", &code])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Stop a fixture we spawned ourselves.
fn kill(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Where termed materializes the embedded server for this test's settings.
fn embedded_script() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_CONFIG_HOME")?;
    Some(Path::new(&dir).join("termed").join("onnx_server.py"))
}

#[test]
fn provider_503_falls_back_to_local_onnx_inference() {
    if common::isolate("provider_503_falls_back_to_local_onnx_inference") {
        return;
    }
    if !have_python() {
        return;
    }
    if !have_onnx_runtime() {
        eprintln!("skipping: python3 lacks onnxruntime/tokenizers (see local_inference/README.md)");
        return;
    }
    let root = std::env::temp_dir().join(format!("termed-local-onnx-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    assert!(fixture_model(), "tiny ONNX fixture model is missing");
    let model = manifest().join("tests/fixtures/tiny_onnx_model");

    let Some((gateway, gateway_port)) = spawn(&manifest().join("tests/fixtures/mock_503_gateway.py"), &[])
    else {
        return;
    };
    let Some((server, server_port)) = spawn(
        &manifest().join("local_inference/onnx_server.py"),
        &["--model", model.to_str().unwrap(), "--port", "0"],
    ) else {
        kill(gateway);
        return;
    };
    if !wait_healthy(&server_port) {
        kill(server);
        kill(gateway);
        panic!("local ONNX server never became ready");
    }

    // SAFETY: single test in this binary; no concurrent environment access.
    unsafe {
        std::env::set_var("TERMED_AGENT", "builtin");
        std::env::set_var(
            "TERMED_LOCAL_ENDPOINT",
            format!("http://127.0.0.1:{server_port}/v1"),
        );
    }
    let calc = root.join("calc.py");
    std::fs::write(&calc, "def add(a, b):\n    return a - b\n").unwrap();
    let mut app = App::new(root.clone(), vec![calc], "Visual Studio Dark", false);
    let options = AgentConfig {
        backend: "builtin".into(),
        provider: "custom".into(),
        endpoint: format!("http://127.0.0.1:{gateway_port}"),
        model: "unavailable/model:free".into(),
        ..Default::default()
    };
    app.preferences.agent = options.clone();
    app.agent.configure(options, String::new()).unwrap();
    app.run(Cmd::AgentToggle);
    for c in "hello local".chars() {
        app.handle_event(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )));
    }
    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    // Drive the app until the turn ends. Waiting on `usage` would hang: a
    // provider that only ever answers 503 never streams a usage update.
    let t = Instant::now();
    while app.agent.is_working() {
        app.pump_agent();
        std::thread::sleep(Duration::from_millis(20));
        assert!(
            t.elapsed() < Duration::from_secs(120),
            "the turn never finished: {:?}",
            app.agent.entries
        );
    }
    // One more drain so the final transcript entries are present.
    app.pump_agent();
    let script = embedded_script();
    // The agent adopts the already-running server instead of starting one;
    // shut that (or whatever it started) down cleanly, then the fixture.
    shutdown(&server_port);
    let stopped = wait_down(&server_port);
    kill(server);
    kill(gateway);
    assert!(stopped, "the local server should stop on /admin/shutdown");

    // Normalize entries to text: notices and assistant replies both matter.
    let transcript = app
        .agent
        .entries
        .iter()
        .map(|e| format!("{e:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        transcript.contains("local ONNX"),
        "503 should announce the local fallback:\n{transcript}"
    );
    assert!(
        app.agent.entries.iter().any(
            |e| matches!(e, Entry::Agent(t) if !t.trim().is_empty())
        ),
        "the local model should answer:\n{transcript}"
    );
    assert!(
        !transcript.contains("Provider request failed"),
        "the turn must not fail after a 503:\n{transcript}"
    );
    assert!(
        script.is_some_and(|p| p.exists()),
        "the embedded server should be materialized next to settings"
    );
    app.agent.stop();
}
