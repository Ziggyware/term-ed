mod common;

use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use termed::agent::{Entry, State};
use termed::app::App;
use termed::config::{AgentConfig, CommandPolicy, Preferences};
use termed::supervision::Status;

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn project(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "termed-supervision-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn router_shaped_stream_runs_generated_function_child_and_grandchild_with_correct_context() {
    if common::isolate(
        "router_shaped_stream_runs_generated_function_child_and_grandchild_with_correct_context",
    ) {
        return;
    }
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock_supervised_gateway.py");
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
    let root = project("nested");
    std::fs::write(root.join("notes.txt"), "evidence-marker\n").unwrap();
    std::fs::create_dir_all(root.join(".termed/skills/project")).unwrap();
    std::fs::write(
        root.join(".termed/skills/project/SKILL.md"),
        "# Project\nINHERITED-SKILL-MARKER: cite evidence and distinguish checks actually run.\n",
    )
    .unwrap();
    let mut prefs = Preferences::default();
    prefs.agent = AgentConfig {
        backend: "builtin".into(),
        provider: "custom".into(),
        endpoint: format!("http://127.0.0.1:{}/v1", port.trim()),
        model: "test/router-contract".into(),
        instructions:
            "ROOT-RULE-MARKER: Preserve the root goal and never claim checks not actually run."
                .into(),
        skills: vec!["project".into()],
        read_only: true,
        commands: CommandPolicy::Deny,
        plugins: false,
        ..Default::default()
    };
    prefs.sub_agents.iter_mut().find(|p|p.id=="coordinator").unwrap().agent.instructions="COORDINATOR-RULE-MARKER: Collect a focused reviewer result, preserve constraints and scope, and verify evidence.".into();
    prefs
        .sub_agents
        .iter_mut()
        .find(|p| p.id == "coordinator")
        .unwrap()
        .agent
        .max_output_tokens = 1024;
    prefs.sub_agents.iter_mut().find(|p|p.id=="reviewer").unwrap().agent.instructions="LEAF-RULE-MARKER: Inspect live notes and cite the exact evidence; do not edit or execute commands.".into();
    prefs.save(&root).unwrap();
    let mut app = App::new(
        root.clone(),
        vec![root.join("notes.txt")],
        "Visual Studio Dark",
        false,
    );
    app.agent.input="ROOT-GOAL-MARKER: Investigate the notes with a coordinator and reviewer; preserve evidence and do not modify files.".into();
    app.agent_send();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut changed_live = false;
    while app.agent.state != State::Ready
        || !app
            .agent
            .entries
            .iter()
            .any(|e| matches!(e,Entry::Agent(s) if s.contains("Nested audit finished")))
    {
        assert!(
            Instant::now() < deadline,
            "nested task stalled: {:?} {:?}",
            app.agent.state,
            app.agent.entries
        );
        app.pump_agent();
        app.pump_plugins();
        if !changed_live&&app.agent.entries.iter().any(|e|matches!(e,Entry::Notice{text,..} if text.contains("Preparing a bounded evidence review"))) {
            app.editor_mut().unwrap().replace_document("evidence-marker\nunsaved-live-marker\n");changed_live=true;
        }
        assert!(
            app.agent.pending_permission().is_none(),
            "read-only tree requested execution"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(changed_live);
    assert_eq!(app.automation.task_count(), 0);
    let records = &app.automation.overwatch.records;
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].depth, 1);
    assert_eq!(records[1].depth, 2);
    assert_eq!(records[1].parent_id, Some(records[0].task_id));
    assert!(
        records
            .iter()
            .all(|r| r.status == Status::Completed && r.result.is_some())
    );
    assert!(app.editor().unwrap().doc.is_dirty());
    assert_eq!(
        std::fs::read_to_string(root.join("notes.txt")).unwrap(),
        "evidence-marker\n"
    );
    assert!(
        app.agent
            .entries
            .iter()
            .any(|e| matches!(e,Entry::Thought(s) if s.contains("Checking task scope")))
    );
}

#[test]
fn cli_key_setup_is_user_local_private_and_not_put_in_workspace_settings() {
    if common::isolate("cli_key_setup_is_user_local_private_and_not_put_in_workspace_settings") {
        return;
    }
    let root = project("credential-cli");
    let config = root.join("config");
    let key = "test-only-not-a-real-router-credential";
    let mut child = Command::new(env!("CARGO_BIN_EXE_termed"))
        .arg("--router-login")
        .env("XDG_CONFIG_HOME", &config)
        .env_remove("ROUTER_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{key}\n").as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!logs.contains(key));
    let secret = config.join("termed/credentials.json");
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&secret).unwrap()).unwrap();
    assert_eq!(value["router_api_key"], key);
    assert!(!root.join(".termed/settings.json").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&secret).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_termed"))
        .arg("--router-logout")
        .env("XDG_CONFIG_HOME", &config)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!secret.exists());
}

#[test]
fn nested_inheritance_keeps_instructions_skills_provider_and_all_restrictions() {
    if common::isolate("nested_inheritance_keeps_instructions_skills_provider_and_all_restrictions")
    {
        return;
    }
    let mut root = AgentConfig {
        instructions: "ROOT scope".into(),
        skills: vec!["root-skill".into()],
        read_only: true,
        plugins: false,
        commands: CommandPolicy::Deny,
        max_delegations: 3,
        max_steps: 10,
        max_output_tokens: 2048,
        ..Default::default()
    };
    root.use_router();
    let coordinator = AgentConfig {
        instructions: "COORDINATOR role".into(),
        sub_agents: true,
        skills: vec!["coord-skill".into()],
        ..Default::default()
    }
    .restricted_by(&root);
    let leaf = AgentConfig {
        instructions: "LEAF role".into(),
        sub_agents: false,
        ..Default::default()
    }
    .restricted_by(&coordinator);
    assert_eq!(
        leaf.inherited_instructions,
        vec!["ROOT scope", "COORDINATOR role"]
    );
    assert_eq!(leaf.skills, vec!["root-skill", "coord-skill"]);
    assert!(leaf.read_only && leaf.uses_router());
    assert!(!leaf.plugins && !leaf.sub_agents);
    assert_eq!(leaf.commands, CommandPolicy::Deny);
    assert_eq!(leaf.max_steps, 10);
    assert_eq!(leaf.max_delegations, 3);
    assert_eq!(leaf.max_output_tokens, 2048);
    let serialized = serde_json::to_value(&leaf).unwrap();
    assert!(serialized.get("inherited_instructions").is_none());
    assert!(serialized.get("router_api_key").is_none());
    let foreign = AgentConfig {
        provider: "custom".into(),
        endpoint: "https://different.invalid/v1".into(),
        ..Default::default()
    }
    .restricted_by(&root);
    assert!(!foreign.uses_router());
    assert!(foreign.api_key_env.is_empty());
    assert!(
        AgentConfig {
            provider: "router".into(),
            endpoint: "https://different.invalid/v1".into(),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        AgentConfig {
            api_key_env: "ROUTER_API_KEY".into(),
            endpoint: "https://different.invalid/v1".into(),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert_eq!(json!(coordinator.max_agent_depth), 2);
}

#[test]
fn saved_router_key_resolves_privately_env_overrides_and_file_tools_cannot_read_it() {
    if common::isolate(
        "saved_router_key_resolves_privately_env_overrides_and_file_tools_cannot_read_it",
    ) {
        return;
    }
    const KEY: &str = "test-only-persisted-router-key";
    if std::env::var_os("TERMED_KEY_RESOLUTION_CHILD").is_some() {
        let root = PathBuf::from(std::env::var_os("TERMED_TEST_PRIVATE_ROOT").unwrap());
        let mut options = AgentConfig::default();
        options.use_router();
        let key = termed::provider::authorization(&options, termed::provider::ROUTER_ENDPOINT)
            .unwrap()
            .unwrap();
        let expected = if std::env::var_os("ROUTER_API_KEY").is_some() {
            "test-only-environment-override"
        } else {
            KEY
        };
        assert_eq!(key.expose(), expected);
        assert!(!format!("{key:?}").contains(expected));
        assert_eq!(
            termed::credentials::Store::default()
                .load()
                .unwrap()
                .unwrap()
                .expose(),
            KEY
        );
        assert!(termed::provider::authorization(&options, "https://other.invalid/v1").is_err());
        let path = termed::credentials::path();
        assert!(termed::config::workspace_path(&root, &path.to_string_lossy()).is_err());
        assert!(termed::search::TextFile::read(&path).is_none());
        let mut app = App::new(
            root.clone(),
            vec![path.clone()],
            "Visual Studio Dark",
            false,
        );
        assert!(app.editors.is_empty());
        #[cfg(unix)]
        {
            let alias = root.join("alias.json");
            std::os::unix::fs::symlink(&path, &alias).unwrap();
            assert!(termed::config::workspace_path(&root, "alias.json").is_err());
            assert!(termed::search::TextFile::read(&alias).is_none());
            app.open_file(&alias);
            assert!(app.editors.is_empty());
            let hard = root.join("hard-alias.json");
            std::fs::hard_link(&path, &hard).unwrap();
            assert!(termed::credentials::is_private_path(&hard));
            assert!(termed::search::TextFile::read(&hard).is_none());
            std::fs::remove_file(hard).unwrap();
            std::fs::remove_file(alias).unwrap();
        }
        return;
    }
    let root = project("resolve-key");
    let config = root.join("config");
    let mut child = Command::new(env!("CARGO_BIN_EXE_termed"))
        .arg("--router-login")
        .env("XDG_CONFIG_HOME", &config)
        .env("ROUTER_API_KEY", KEY)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert!(child.wait().unwrap().success());
    for environment_override in [false, true] {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "saved_router_key_resolves_privately_env_overrides_and_file_tools_cannot_read_it",
            ])
            .env("TERMED_KEY_RESOLUTION_CHILD", "1")
            .env("TERMED_TEST_PRIVATE_ROOT", &root)
            .env("XDG_CONFIG_HOME", &config)
            .env_remove("ROUTER_API_KEY");
        if environment_override {
            command.env("ROUTER_API_KEY", "test-only-environment-override");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains(KEY));
    }
}

#[test]
fn https_bearer_auth_redacts_errors_and_refuses_cross_origin_redirects() {
    if common::isolate("https_bearer_auth_redacts_errors_and_refuses_cross_origin_redirects") {
        return;
    }
    if std::env::var_os("TERMED_SECURITY_CHILD").is_some() {
        let root = PathBuf::from(std::env::var_os("TERMED_SECURITY_ROOT").unwrap());
        let mode = std::env::var("TERMED_SECURITY_MODE").unwrap();
        let captured = root.join(format!("captured-{mode}.json"));
        let leaked = root.join(format!("leaked-{mode}.txt"));
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mock_provider_security.py");
        let mut server = Command::new("python3")
            .arg(fixture)
            .arg(&mode)
            .arg(root.join("cert.pem"))
            .arg(root.join("key.pem"))
            .arg(&leaked)
            .arg(&captured)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut port = String::new();
        BufReader::new(server.stdout.take().unwrap())
            .read_line(&mut port)
            .unwrap();
        let _server = Server(server);
        let mut prefs = Preferences::default();
        prefs.agent = AgentConfig {
            backend: "builtin".into(),
            provider: "custom".into(),
            endpoint: format!("https://127.0.0.1:{}/v1", port.trim()),
            api_key_env: "TERMED_TEST_PROVIDER_KEY".into(),
            model: "test/security".into(),
            ..Default::default()
        };
        prefs.save(&root).unwrap();
        let mut app = App::new(root.clone(), vec![], "Visual Studio Dark", false);
        app.agent.input = "Check the local provider contract.".into();
        app.agent_send();
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.agent.state != State::Ready {
            assert!(
                Instant::now() < deadline,
                "security request stalled: {:?}",
                app.agent.entries
            );
            app.pump_agent();
            std::thread::sleep(Duration::from_millis(5));
        }
        let capture: serde_json::Value =
            serde_json::from_slice(&std::fs::read(captured).unwrap_or_else(|e| {
                panic!(
                    "TLS request did not reach provider: {e}; {:?}",
                    app.agent.entries
                )
            }))
            .unwrap();
        assert_eq!(capture["bearer_ok"], true);
        let transcript = format!("{:?}", app.agent.entries);
        assert!(!transcript.contains("test-only-local-provider-secret"));
        if mode == "auth" {
            assert!(transcript.contains("[REDACTED]"));
            assert!(transcript.contains("Authentication failed"));
        }
        assert!(
            !leaked.exists(),
            "A request followed the redirect to another origin"
        );
        return;
    }
    let root = project("https-security");
    let status = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "1",
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=DNS:localhost,IP:127.0.0.1",
            "-addext",
            "basicConstraints=critical,CA:FALSE",
            "-keyout",
        ])
        .arg(root.join("key.pem"))
        .arg("-out")
        .arg(root.join("cert.pem"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("openssl generates local test certificate");
    assert!(status.success());
    for mode in ["auth", "redirect"] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "https_bearer_auth_redacts_errors_and_refuses_cross_origin_redirects",
            ])
            .env("TERMED_SECURITY_CHILD", "1")
            .env("TERMED_SECURITY_ROOT", &root)
            .env("TERMED_SECURITY_MODE", mode)
            .env("SSL_CERT_FILE", root.join("cert.pem"))
            .env(
                "TERMED_TEST_PROVIDER_KEY",
                "test-only-local-provider-secret",
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "TLS {mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
