mod common;

use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use termed::agent::{Entry, State};
use termed::app::App;
use termed::config::{AgentConfig, CommandPolicy, Preferences};
use termed::supervision::Status;

/// Children that have reached the gateway; each waits here for its siblings.
type Barrier = Arc<(Mutex<HashSet<String>>, Condvar)>;

fn sse(delta: Value, finish: &str) -> Vec<u8> {
    let events = [
        json!({"choices":[{"delta":delta,"finish_reason":null}]}),
        json!({"choices":[{"delta":{},"finish_reason":finish}]}),
    ];
    let mut body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
    body.push_str("data: [DONE]\n\n");
    let mut out = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend(body.into_bytes());
    out
}

fn calls(calls: &[(&str, Value)]) -> Vec<u8> {
    let calls: Vec<_> = calls
        .iter()
        .enumerate()
        .map(|(i, (name, args))| json!({"id":format!("c{i}-{name}"),"type":"function","function":{"name":name,"arguments":args.to_string()}}))
        .collect();
    sse(json!({"tool_calls":calls}), "tool_calls")
}

fn reject(why: &str) -> Vec<u8> {
    let body = json!({"error":{"message":why}}).to_string();
    format!("HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
}

fn respond(request: &Value, barrier: &Barrier) -> Vec<u8> {
    let messages = request["messages"].as_array().unwrap();
    let user = messages.iter().find(|m| m["role"] == "user").unwrap()["content"].as_str().unwrap();
    let results: Vec<&str> = messages
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["content"].as_str().unwrap_or(""))
        .collect();
    let Some(brief) = user.split_once("<task_brief>\n").and_then(|(_, b)| b.rsplit_once("\n</task_brief>")).map(|(b, _)| b) else {
        // Root: fan out two delegations in one response, then join.
        return match results.len() {
            0 => calls(&[
                ("delegate_task", json!({"agent":"reviewer","task":"Review alpha."})),
                ("delegate_task", json!({"agent":"tester","task":"Plan beta."})),
            ]),
            2 if results.iter().all(|r| r.contains("\"status\":\"completed\"")) => {
                sse(json!({"content":"Fan-out joined both workers."}), "stop")
            }
            _ => reject(&format!("root expected two completed results, got {results:?}")),
        };
    };
    let objective = serde_json::from_str::<Value>(brief).unwrap()["objective"].as_str().unwrap().to_string();
    match results.len() {
        0 => {
            // Serialized delegation would leave the first child here alone.
            let (lock, cv) = &**barrier;
            let mut arrived = lock.lock().unwrap();
            arrived.insert(objective);
            cv.notify_all();
            let (arrived, timeout) = cv
                .wait_timeout_while(arrived, Duration::from_secs(10), |a| a.len() < 2)
                .unwrap();
            if timeout.timed_out() {
                return reject(&format!("children were serialized: {arrived:?}"));
            }
            calls(&[("list_files", json!({}))])
        }
        1 if request.get("tools").is_none()
            && messages.last().unwrap()["content"].as_str().unwrap_or("").contains("step budget") =>
        {
            sse(json!({"content":format!("Finalized answer for {objective}")}), "stop")
        }
        _ => reject("child exceeded its step budget without a tool-free final round"),
    }
}

fn serve(stream: TcpStream, barrier: Barrier) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse().unwrap();
        }
        if line == "\r\n" {
            break;
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let request: Value = serde_json::from_slice(&body).unwrap();
    let _ = (&stream).write_all(&respond(&request, &barrier));
}

#[test]
fn sibling_delegations_run_concurrently_and_exhausted_workers_still_answer() {
    if common::isolate("sibling_delegations_run_concurrently_and_exhausted_workers_still_answer") {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let barrier: Barrier = Default::default();
    let shared = barrier.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let barrier = shared.clone();
            std::thread::spawn(move || serve(stream, barrier));
        }
    });
    let root = std::env::temp_dir().join(format!("termed-ultra-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("notes.txt"), "evidence\n").unwrap();
    let mut prefs = Preferences::default();
    prefs.agent = AgentConfig {
        backend: "builtin".into(),
        provider: "custom".into(),
        endpoint: format!("http://127.0.0.1:{port}/v1"),
        model: "test/ultra".into(),
        read_only: true,
        commands: CommandPolicy::Deny,
        plugins: false,
        ..Default::default()
    };
    for profile in &mut prefs.sub_agents {
        profile.agent.max_steps = 1;
    }
    prefs.save(&root).unwrap();
    let mut app = App::new(root.clone(), vec![root.join("notes.txt")], "Visual Studio Dark", false);
    app.agent.input = "Review alpha and plan beta in parallel.".into();
    app.agent_send();
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.agent.state != State::Ready
        || !app.agent.entries.iter().any(|e| matches!(e, Entry::Agent(s) if s.contains("Fan-out joined")))
    {
        assert!(Instant::now() < deadline, "fan-out stalled: {:?} {:?}", app.agent.state, app.agent.entries);
        app.pump_agent();
        app.pump_plugins();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(barrier.0.lock().unwrap().len(), 2);
    let records = &app.automation.overwatch.records;
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|r| r.status == Status::Completed
        && r.result.as_deref().is_some_and(|s| s.contains("Finalized answer"))));
}
