//! End-to-end provider compatibility without using a live router credential.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use termed::agent::{Entry, State};
use termed::app::App;
use termed::config::{AgentConfig, Preferences};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn router_compatible_provider_handles_json_and_streamed_tool_calls() {
    if let Some(root) = std::env::var_os("TERMED_COMPAT_CHILD_ROOT") {
        let root = PathBuf::from(root);
        let port = std::env::var("TERMED_COMPAT_PORT").unwrap();
        let file = root.join("calc.py");
        std::fs::write(&file, "def add(a, b):\n    return a - b\n").unwrap();
        let mut prefs = Preferences::default();
        prefs.agent = AgentConfig {
            backend: "builtin".into(),
            endpoint: format!("http://127.0.0.1:{port}/v1"),
            api_key_env: "TERMED_COMPAT_TEST_KEY".into(),
            model: "free:code".into(),
            ..Default::default()
        };
        prefs.save(&root).unwrap();
        let mut app = App::new(root, vec![file.clone()], "Visual Studio Dark", false);
        app.agent.input = "Fix add using the tools.".into();
        app.agent_send();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            app.pump_agent();
            if app.agent.state == State::Ready {
                if app
                    .agent
                    .entries
                    .iter()
                    .any(|e| matches!(e, Entry::Agent(s) if s.contains("The addition is fixed")))
                {
                    break;
                }
                assert!(
                    !app.agent
                        .entries
                        .iter()
                        .any(|e| matches!(e, Entry::Notice { error: true, .. })),
                    "provider failed: {:?}",
                    app.agent.entries
                );
            }
            assert!(
                Instant::now() < deadline,
                "provider stalled: {:?}",
                app.agent.entries
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "def add(a, b):\n    return a + b\n"
        );
        assert_eq!(
            app.editor().unwrap().doc.rope.to_string(),
            "def add(a, b):\n    return a + b\n"
        );
        let ids: Vec<_> = app
            .agent
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Tool(tool) => Some(&tool.id),
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 2, "both tool calls should be visible");
        assert_ne!(ids[0], ids[1]);
        assert!(!format!("{:?}", app.agent.entries).contains("test-only-key+plus"));
        app.agent.stop();
        return;
    }

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mock_router_compat_gateway.py");
    let mut server = Command::new("python3")
        .arg(fixture)
        .stdout(Stdio::piped())
        .spawn()
        .expect("python fixture starts");
    let mut port = String::new();
    BufReader::new(server.stdout.take().unwrap())
        .read_line(&mut port)
        .unwrap();
    let _server = Server(server);
    let root = std::env::temp_dir().join(format!("termed-router-compat-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "router_compatible_provider_handles_json_and_streamed_tool_calls",
        ])
        .env("TERMED_COMPAT_CHILD_ROOT", &root)
        .env("TERMED_COMPAT_PORT", port.trim())
        .env("TERMED_COMPAT_TEST_KEY", "test-only-key+plus")
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env_remove("ROUTER_API_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
