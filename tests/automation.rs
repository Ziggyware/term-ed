mod common;

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use termed::agent::{Entry, State};
use termed::app::App;
use termed::config::Preferences;
use termed::tool_bridge::Bridge;

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn built_in_agent_uses_live_plugins_skills_and_an_isolated_specialist() {
    if common::isolate("built_in_agent_uses_live_plugins_skills_and_an_isolated_specialist") {
        return;
    }
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock_automation_gateway.py");
    let Ok(mut server) = Command::new("python3")
        .arg(fixture)
        .stdout(Stdio::piped())
        .spawn()
    else {
        return;
    };
    let mut port = String::new();
    BufReader::new(server.stdout.take().unwrap())
        .read_line(&mut port)
        .unwrap();
    let _server = Server(server);
    let root = std::env::temp_dir().join(format!("termed-automation-e2e-{}", std::process::id()));
    std::fs::create_dir_all(root.join(".termed/skills/policy")).unwrap();
    std::fs::write(root.join("notes.txt"), "beta\nalpha\n").unwrap();
    std::fs::write(
        root.join(".termed/skills/policy/SKILL.md"),
        "# Policy\n\nSKILL-MARKER: verify changes carefully.\n",
    )
    .unwrap();
    let mut prefs = Preferences::default();
    prefs.agent.backend = "builtin".into();
    prefs.agent.provider = "custom".into();
    prefs.agent.endpoint = format!("http://127.0.0.1:{}", port.trim());
    prefs.agent.model = "test/automation".into();
    prefs.agent.instructions = "PARENT-AUTOMATION".into();
    prefs.agent.skills = vec!["policy".into()];
    prefs.sub_agents[0].agent.instructions = "SPECIALIST-REVIEW".into();
    prefs.sub_agents[0].agent.skills = vec!["policy".into()];
    prefs.save(&root).unwrap();
    let mut app = App::new(
        root.clone(),
        vec![root.join("notes.txt")],
        "Visual Studio Dark",
        false,
    );
    app.agent.input = "Use plugins and the reviewer to inspect these notes.".into();
    app.agent_send();
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut approved = false;
    while app.agent.state != State::Ready
        || !app
            .agent
            .entries
            .iter()
            .any(|e| matches!(e,Entry::Agent(text) if text.contains("Finished:")))
    {
        assert!(
            Instant::now() < deadline,
            "agent stalled: {:?} {:?}",
            app.agent.state,
            app.agent.entries
        );
        app.pump_agent();
        app.pump_plugins();
        if let Some(index) = app.agent.pending_permission() {
            assert!(
                !approved,
                "unexpected extra approval: {:?}",
                app.agent.entries
            );
            assert_eq!(
                std::fs::read_to_string(root.join("notes.txt")).unwrap(),
                "beta\nalpha\n"
            );
            app.agent.choose(index, Some("allow"));
            approved = true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(approved);
    assert_eq!(app.editor().unwrap().doc.rope.to_string(), "alpha\nbeta\n");
    assert!(app.editor().unwrap().doc.is_dirty());
    assert_eq!(
        std::fs::read_to_string(root.join("notes.txt")).unwrap(),
        "beta\nalpha\n"
    );
    assert!(
        app.agent
            .entries
            .iter()
            .any(|e| matches!(e,Entry::Tool(t) if t.title.starts_with("Reviewer:")))
    );
    assert_eq!(app.automation.task_count(), 0);
    assert!(!app.automation.busy());
    app.run(termed::commands::Command::Undo);
    assert_eq!(app.editor().unwrap().doc.rope.to_string(), "beta\nalpha\n");
    app.agent.stop();
}

#[test]
fn external_acp_agents_get_an_authenticated_mcp_stdio_relay() {
    if common::isolate("external_acp_agents_get_an_authenticated_mcp_stdio_relay") {
        return;
    }
    let bridge = Bridge::start().unwrap();
    let spec = bridge.mcp_server().unwrap();
    assert!(bridge.address.ip().is_loopback());
    let mut relay = Command::new(env!("CARGO_BIN_EXE_termed"))
        .args(["--mcp-bridge", &bridge.address.to_string()])
        .env(
            "TERMED_BRIDGE_TOKEN",
            spec["env"][0]["value"].as_str().unwrap(),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = relay.stdin.take().unwrap();
    let mut output = BufReader::new(relay.stdout.take().unwrap());
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["result"]["serverInfo"]["name"], "termed");
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})
    )
    .unwrap();
    input.flush().unwrap();
    let request = bridge
        .requests
        .recv_timeout(Duration::from_secs(3))
        .unwrap();
    assert_eq!(request.method, "tools/list");
    request
        .reply
        .send(Ok(
            json!({"tools":termed::tool_bridge::tools(&Default::default())}),
        ))
        .unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert!(
        response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "run_plugin")
    );
    drop(input);
    let status = relay.wait().unwrap();
    assert!(status.success());
}
