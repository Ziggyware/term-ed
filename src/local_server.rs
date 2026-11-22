//! On-device ONNX inference server lifecycle.
//!
//! termed embeds a single-file Python server (`local_inference/onnx_server.py`)
//! that serves an OpenAI-compatible `/v1` API from an ONNX model via ONNX
//! Runtime. This module materializes it into the user config directory and,
//! when the local endpoint is on loopback and not yet serving, starts it
//! with the system Python and waits for `/healthz`. A server that is already
//! running (for example from a previous termed session) is reused; a stale
//! one from an older termed version is asked to shut down first.
//!
//! The server is deliberately not tied to termed's lifetime: it keeps
//! running after termed exits so the next launch reuses it. Stop it with
//! `POST /admin/shutdown` or `pkill -f onnx_server.py`.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// Must match `__version__` in `local_inference/onnx_server.py`.
pub const SERVER_VERSION: &str = "1.1.0";
const SERVER_SOURCE: &str = include_str!("../local_inference/onnx_server.py");
/// Model downloaded on first use when none is configured.
pub const DEFAULT_MODEL: &str = "onnx-community/SmolLM2-135M-Instruct";
/// First run downloads the model from the network; allow generous time.
const READY_TIMEOUT: Duration = Duration::from_secs(300);
const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// How often a still-loading server reports progress to the agent panel.
const PROGRESS_INTERVAL: Duration = Duration::from_secs(20);

pub fn script_path() -> PathBuf {
    crate::config::config_dir().join("onnx_server.py")
}

pub fn log_path() -> PathBuf {
    crate::config::config_dir().join("onnx-server.log")
}

fn python() -> String {
    if let Ok(p) = std::env::var("TERMED_PYTHON")
        && !p.trim().is_empty()
    {
        return p;
    }
    if cfg!(windows) {
        "python".into()
    } else {
        "python3".into()
    }
}

/// Write the embedded server next to the user settings, replacing it when
/// termed was upgraded and the embedded copy changed.
fn materialize() -> Result<PathBuf, String> {
    let path = script_path();
    let parent = path.parent().ok_or("User settings path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    if current != SERVER_SOURCE {
        std::fs::write(&path, SERVER_SOURCE)
            .map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
    }
    Ok(path)
}

/// `(host, port)` for a loopback `http://` endpoint with an explicit port.
/// Anything else (remote host, HTTPS, no port) is never auto-started.
fn loopback_host_port(endpoint: &str) -> Option<(String, u16)> {
    let authority = endpoint.strip_prefix("http://")?.split('/').next().unwrap_or("");
    let (host, port) = authority.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let host = host.trim_matches(['[', ']']);
    if matches!(host, "127.0.0.1" | "localhost" | "::1") {
        Some((host.to_string(), port))
    } else {
        None
    }
}

/// Parsed `/healthz` body. The server answers 503 while loading or after
/// a failed load, so the body is parsed regardless of status.
fn health(http: &ureq::Agent, endpoint: &str) -> Option<Value> {
    let response = http
        .get(format!("{endpoint}/healthz"))
        .config()
        .timeout_per_call(Some(Duration::from_secs(2)))
        .build()
        .call()
        .ok()?;
    let mut text = String::new();
    response
        .into_body()
        .into_reader()
        .take(64 * 1024)
        .read_to_string(&mut text)
        .ok()?;
    serde_json::from_str(&text).ok()
}

/// Health/status probe used by the agent's `local_inference` tool: reads the
/// server's `/healthz` body without starting (or waiting on) anything.
/// Returns `None` when nothing is answering on the endpoint.
pub fn probe(http: &ureq::Agent, endpoint: &str) -> Option<serde_json::Value> {
    health(http, endpoint)
}

/// Port of a loopback `http://` endpoint, for `--local-serve`.
pub fn endpoint_port(endpoint: &str) -> Option<u16> {
    loopback_host_port(endpoint).map(|(_, port)| port)
}

fn ready_error(info: &Value) -> Option<String> {
    info.get("error").and_then(Value::as_str).map(str::to_string)
}

/// The model id advertised by `/v1/models`, without a trailing `/v1` some
/// servers append to their own base URL.
fn advertised_model(list: &Value) -> Option<String> {
    let id = list
        .get("data")?
        .as_array()?
        .first()?
        .get("id")?
        .as_str()?
        .trim()
        .trim_end_matches('/');
    let id = id.strip_suffix("/v1").unwrap_or(id);
    (!id.is_empty()).then(|| id.to_string())
}

/// `POST /admin/shutdown`, used to retire a server from another termed
/// version (or a failed load) before starting the embedded one.
fn shutdown(http: &ureq::Agent, endpoint: &str) {
    let mut urls = vec![format!("{endpoint}/admin/shutdown")];
    if let Some(root) = endpoint.strip_suffix("/v1") {
        // Older 1.0.0 servers only routed `/admin/shutdown` at the root.
        urls.push(format!("{root}/admin/shutdown"));
    }
    for url in urls {
        let _ = http
            .post(url)
            .config()
            .timeout_per_call(Some(Duration::from_secs(2)))
            .build()
            // Bodyless POST: `RequestBuilder<WithBody>` has `send`, not `call`
            // (ureq 3.x). An empty JSON object keeps the request simple; the
            // server ignores the body for this route.
            .send("{}");
    }
}

/// Ask the running server which model it loaded.
fn running_model(http: &ureq::Agent, endpoint: &str) -> Option<String> {
    let response = http
        .get(format!("{endpoint}/models"))
        .config()
        .timeout_per_call(Some(Duration::from_secs(5)))
        .build()
        .call()
        .ok()?;
    let mut text = String::new();
    response
        .into_body()
        .into_reader()
        .take(256 * 1024)
        .read_to_string(&mut text)
        .ok()?;
    advertised_model(&serde_json::from_str(&text).ok()?)
}

/// Poll `/healthz` until the server is ready, failed, or the timeout
/// elapses. `child` is the process we spawned, if any, for fail-fast exit.
fn wait_ready<F: Fn() -> bool, P: Fn(Duration)>(
    http: &ureq::Agent,
    endpoint: &str,
    model: &str,
    mut child: Option<std::process::Child>,
    cancelled: F,
    progress: P,
) -> Result<String, String> {
    let start = Instant::now();
    let deadline = start + READY_TIMEOUT;
    let mut last_report = Duration::ZERO;
    while Instant::now() < deadline {
        if cancelled() {
            if let Some(child) = child.as_mut() {
                // Startup is cheap to redo; a half-loaded 500 MB model is not
                // worth leaving behind on a machine that may be low on RAM.
                let _ = child.kill();
            }
            return Err("Local ONNX startup was cancelled".into());
        }
        if let Some(child) = child.as_mut()
            && let Some(status) = child.try_wait().map_err(|e| e.to_string())?
        {
            let log = std::fs::read_to_string(log_path()).unwrap_or_default();
            let hint = if log.contains("ModuleNotFoundError") || log.contains("No module named") {
                " ONNX Runtime is not installed for this Python: run \
                 pip install -r local_inference/requirements.txt."
            } else if log.contains("Address already in use") {
                " That port is already serving something else; set another Local \
                 ONNX endpoint (default http://127.0.0.1:8080/v1)."
            } else {
                ""
            };
            return Err(format!(
                "Local ONNX server exited with {status} while loading {model}.{hint} \
                 See {}",
                log_path().display()
            ));
        }
        if let Some(info) = health(http, endpoint) {
            if info.get("status").and_then(Value::as_str) == Some("ok") {
                return Ok(running_model(http, endpoint).unwrap_or_else(|| model.to_string()));
            }
            if let Some(error) = ready_error(&info) {
                if let Some(mut c) = child {
                    // Never leave a failed server bound to the port where it
                    // would answer 503 to every later session.
                    shutdown(http, endpoint);
                    let _ = c.kill();
                    let _ = c.wait();
                }
                return Err(format!(
                    "Local ONNX server failed to load {model}: {error}. See {}",
                    log_path().display()
                ));
            }
        }
        std::thread::sleep(POLL_INTERVAL);
        // First run downloads the model from the network, which can take
        // minutes: report it, and count as activity so the turn survives.
        let elapsed = start.elapsed();
        if elapsed >= last_report + PROGRESS_INTERVAL {
            last_report = elapsed;
            progress(elapsed);
        }
    }
    Err(format!(
        "Local ONNX server did not become ready within {}s. See {}",
        READY_TIMEOUT.as_secs(),
        log_path().display()
    ))
}

/// The model to load: the chat model the worker is using, unless it is the
/// placeholder id or a remote provider's model. Remote model ids (free-tier
/// aliases and router-forward ids) mean nothing on this machine: an
/// `owner/name` string that is not a Hugging Face repo id would be mistaken
/// for one and fail the first-run fetch with a confusing error, so they all
/// fall back to the default local model.
pub fn model_for(model: &str) -> String {
    let model = model.trim();
    if model.is_empty()
        || model == crate::provider::LOCAL_MODEL
        || model == crate::provider::ROUTER_MODEL
        || model == crate::provider::KONO_MODEL
        || model == crate::builtin_agent::DEFAULT_MODEL
        || model.ends_with(":free")
        || model.starts_with("free:")
        || model == "openrouter/free"
    {
        DEFAULT_MODEL.into()
    } else {
        model.into()
    }
}

/// Ensure a local ONNX server answers at `endpoint`, starting one when it
/// is a loopback endpoint and nothing is listening. Returns the model id to
/// put in requests. `cancelled` aborts the wait for a still-loading server;
/// `progress` receives the elapsed wait every [`PROGRESS_INTERVAL`] so a long
/// first-run model download is reported instead of looking like a stalled
/// turn to the supervisor.
pub fn ensure_running<F: Fn() -> bool, P: Fn(Duration)>(
    http: &ureq::Agent,
    endpoint: &str,
    model: &str,
    cancelled: F,
    progress: P,
) -> Result<String, String> {
    let trimmed = model.trim();
    let model = model_for(trimmed);
    let explicit_model = !trimmed.is_empty() && trimmed != DEFAULT_MODEL && model == trimmed;
    let is_loopback = loopback_host_port(endpoint).is_some();
    if is_loopback {
        let _ = materialize();
    }
    if let Some(info) = health(http, endpoint) {
        let same_version = info.get("version").and_then(Value::as_str) == Some(SERVER_VERSION);
        let wrong_model = explicit_model
            && is_loopback
            && info
                .get("model")
                .and_then(Value::as_str)
                .is_some_and(|loaded| loaded != model);
        if same_version && !wrong_model {
            if info.get("status").and_then(Value::as_str) == Some("ok") {
                return Ok(running_model(http, endpoint).unwrap_or(model));
            }
            if let Some(error) = ready_error(&info) {
                if !is_loopback {
                    return Err(format!(
                        "Local ONNX server failed to load {model}: {error}. See {}",
                        log_path().display()
                    ));
                }
                // A loopback server wedged in "failed" state from an earlier
                // attempt must not poison later turns: shut it down below and
                // start a fresh one.
            } else {
                // Already loading (started by another termed instance): wait
                // for it instead of spawning a second server.
                return wait_ready(http, endpoint, &model, None, cancelled, progress);
            }
        }
        // A server from an older termed version, with a different model, or
        // wedged in "failed" state: ask it to stop, then start a fresh one.
        shutdown(http, endpoint);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && health(http, endpoint).is_some() {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let Some((host, port)) = loopback_host_port(endpoint) else {
        return Err(format!(
            "Local ONNX server is not reachable at {endpoint}. Start it with: \
             python3 local_inference/onnx_server.py --model {model} \
             (see docs/local-inference.md)"
        ));
    };
    let script = materialize()?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
        .map_err(|e| format!("Cannot open {}: {e}", log_path().display()))?;
    let child = Command::new(python())
        .arg(&script)
        .arg("--model")
        .arg(&model)
        .arg("--host")
        .arg(&host)
        .arg("--port")
        .arg(port.to_string())
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| {
            format!(
                "Could not start the local ONNX server with {}: {e}. \
                 Install its dependencies first: \
                 pip install -r local_inference/requirements.txt \
                 (see docs/local-inference.md)",
                python()
            )
        })?;
    wait_ready(http, endpoint, &model, Some(child), cancelled, progress)
}

/// Download (or verify) the model without starting the server, so a later
/// 503 fallback does not wait on a cold download mid-outage. Needs only
/// Python's stdlib: ONNX Runtime is not imported for this.
pub fn prefetch(model: &str) -> Result<(), String> {
    let script = materialize()?;
    // The cache is shared by every endpoint, so only the model matters here.
    eprintln!("Fetching {model} ...");
    let status = Command::new(python())
        .arg(&script)
        .arg("--model")
        .arg(model_for(model))
        .arg("--prewarm")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| format!("Could not run {}: {e}", python()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Could not fetch {model}. Set HF_ENDPOINT to a mirror, or pass \
             --local-model with a directory containing model.onnx and \
             tokenizer.json (see docs/local-inference.md)."
        ))
    }
}

/// Run the embedded server in the foreground (the `--local-serve` entry
/// point), using this Python and the user's model cache.
pub fn serve_foreground(endpoint: &str, model: &str, port: u16) -> Result<(), String> {
    let script = materialize()?;
    let host = loopback_host_port(endpoint)
        .map(|(host, _)| host)
        .unwrap_or_else(|| "127.0.0.1".into());
    let status = Command::new(python())
        .arg(&script)
        .arg("--model")
        .arg(model)
        .arg("--host")
        .arg(host)
        .arg("--port")
        .arg(port.to_string())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| {
            format!(
                "Could not run the local ONNX server with {}: {e}. \
                 Install its dependencies first: \
                 pip install -r local_inference/requirements.txt",
                python()
            )
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Local ONNX server exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_http_endpoints_with_a_port_are_auto_started() {
        assert_eq!(
            loopback_host_port("http://127.0.0.1:8080/v1"),
            Some(("127.0.0.1".into(), 8080))
        );
        assert_eq!(
            loopback_host_port("http://localhost:9090/v1"),
            Some(("localhost".into(), 9090))
        );
        assert_eq!(
            loopback_host_port("http://[::1]:8080/v1"),
            Some(("::1".into(), 8080))
        );
        // Remote hosts, HTTPS and port-less endpoints are never spawned.
        assert_eq!(loopback_host_port("http://192.168.1.20:8080/v1"), None);
        assert_eq!(loopback_host_port("https://127.0.0.1:8080/v1"), None);
        assert_eq!(loopback_host_port("http://127.0.0.1/v1"), None);
        assert_eq!(loopback_host_port("not a url"), None);
    }

    #[test]
    fn placeholder_and_router_models_use_the_default_onnx_model() {
        assert_eq!(model_for(""), DEFAULT_MODEL);
        assert_eq!(model_for(crate::provider::LOCAL_MODEL), DEFAULT_MODEL);
        assert_eq!(model_for(crate::provider::ROUTER_MODEL), DEFAULT_MODEL);
        assert_eq!(model_for("my-model"), "my-model");
        assert_eq!(model_for("/models/qwen"), "/models/qwen");
    }

    #[test]
    fn remote_provider_ids_use_the_default_onnx_model() {
        // Remote model ids mean nothing on this machine; they are meaningless
        // Hugging Face repo names, so they must not reach the first-run fetch.
        for remote in [
            "free:auto",
            "free:code",
            "free:smart",
            "free:tools",
            "kilo-auto/free",
            "openrouter/free",
            "some-nested/model:free",
        ] {
            assert_eq!(model_for(remote), DEFAULT_MODEL, "{remote}");
        }
        // Local directory paths and real repo ids pass through untouched.
        assert_eq!(model_for("~/models/qwen"), "~/models/qwen");
        assert_eq!(
            model_for("onnx-community/SmolLM2-135M-Instruct"),
            "onnx-community/SmolLM2-135M-Instruct"
        );
    }
}
