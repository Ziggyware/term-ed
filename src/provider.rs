//! Explicit provider selection and origin-bound router authentication.
use crate::config::AgentConfig;
use crate::credentials::{ROUTER_ENV, Secret};

pub const ROUTER_ORIGIN: &str = "https://nokey-router.ziggyware.deno.net";
pub const ROUTER_ENDPOINT: &str = "https://nokey-router.ziggyware.deno.net/v1";
pub const ROUTER_MODEL: &str = "free:code";

pub fn pinned_router(endpoint: &str) -> bool {
    matches!(
        endpoint.trim_end_matches('/'),
        ROUTER_ORIGIN | ROUTER_ENDPOINT
    )
}

pub const KONO_ORIGIN: &str = "https://kono-router.onrender.com";
pub const KONO_ENDPOINT: &str = "https://kono-router.onrender.com/v1";
/// Kono routes by lane capability; `free:auto` reaches models that answer in
/// prose without calling tools, so the agent asks for the tool-capable lane.
pub const KONO_MODEL: &str = "free:tools";

/// Default base URL of the on-device ONNX inference server
/// (`local_inference/onnx_server.py`). Loopback only: prompts never leave
/// the machine, and no API key is involved.
pub const LOCAL_ENDPOINT: &str = "http://127.0.0.1:8080/v1";
/// Model id reported when the local server has not advertised one yet.
pub const LOCAL_MODEL: &str = "local-onnx";
/// Environment overrides for the local endpoint, in priority order.
pub const LOCAL_ENDPOINT_ENVS: [&str; 2] = ["TERMED_LOCAL_ENDPOINT", "TERMED_ONNX_ENDPOINT"];

/// Accept the root URLs users see in the Kono and OpenAI-compatible launch
/// output. A bare origin (`https://host`, `http://host:port`, loopback included)
/// gets the conventional `/v1`; any URL with a path is used verbatim, so a
/// deliberately unversioned API or a proxy path keeps working.
pub fn custom_endpoint(input: &str) -> String {
    let base = input.trim().trim_end_matches('/');
    let base = base.strip_suffix("/chat/completions").unwrap_or(base);
    if base == KONO_ORIGIN {
        return KONO_ENDPOINT.into();
    }
    if let Some(authority) = base
        .strip_prefix("http://")
        .or_else(|| base.strip_prefix("https://"))
    {
        let pathless = authority.split_once('/').is_none_or(|(_, path)| path.is_empty());
        let (host, port) = if let Some(rest) = authority.strip_prefix("[::1]") {
            ("[::1]", rest)
        } else {
            let host = authority.split(':').next().unwrap_or("");
            (host, &authority[host.len()..])
        };
        if pathless
            && !host.is_empty()
            && (port.is_empty() || port.strip_prefix(':').is_some_and(|p| p.parse::<u16>().is_ok()))
        {
            return format!("{base}/v1");
        }
    }
    base.into()
}

/// Resolve the local ONNX server base URL: the configured local endpoint,
/// then the environment overrides, then the loopback default. A bare host
/// gets `/v1`, exactly like a custom endpoint.
pub fn local_endpoint(options: &AgentConfig) -> String {
    if !options.local_endpoint.trim().is_empty() {
        return custom_endpoint(&options.local_endpoint);
    }
    for name in LOCAL_ENDPOINT_ENVS {
        if let Ok(value) = std::env::var(name)
            && !value.trim().is_empty()
        {
            return custom_endpoint(&value);
        }
    }
    LOCAL_ENDPOINT.into()
}

pub fn endpoint(options: &AgentConfig) -> String {
    if options.uses_router() {
        ROUTER_ENDPOINT.into()
    } else if options.uses_kono() {
        KONO_ENDPOINT.into()
    } else if options.uses_local() {
        local_endpoint(options)
    } else if options.endpoint.is_empty() {
        std::env::var("TERMED_FREE_GATEWAY")
            .unwrap_or_else(|_| crate::builtin_agent::GATEWAY.into())
    } else {
        custom_endpoint(&options.endpoint)
    }
}

pub fn authorization(options: &AgentConfig, endpoint: &str) -> Result<Option<Secret>, String> {
    // Local inference needs no key, and no credential (above all the router
    // key) must ever be sent to it, even if settings still name one.
    if options.uses_local() {
        return Ok(None);
    }
    if options.uses_router() || options.api_key_env == ROUTER_ENV {
        if endpoint != ROUTER_ENDPOINT {
            return Err("Refusing to send ROUTER_API_KEY to any origin except the pinned HTTPS router /v1 endpoint".into());
        }
        return crate::credentials::router_key().map_err(|e|e.to_string())?
            .map(Some).ok_or_else(||"Router API key is not configured. Use Settings → AI agent configuration → Save Router API Key, or run termed --router-login locally. ROUTER_API_KEY overrides the saved key.".into());
    }
    if options.api_key_env.is_empty() {
        return Ok(None);
    }
    let value = std::env::var(&options.api_key_env).map_err(|_| {
        format!(
            "Set the {} environment variable for this provider",
            options.api_key_env
        )
    })?;
    let key = crate::credentials::secret_from_environment(&value).map_err(|e| e.to_string())?;
    Ok(Some(key))
}

pub fn sanitize(message: &str, key: Option<&Secret>) -> String {
    let text = match key {
        Some(k) => message.replace(k.expose(), "[REDACTED]"),
        None => message.to_string(),
    };
    crate::supervision::excerpt(&text, 2000)
}

pub fn router_models() -> Vec<(String, String)> {
    [
        ("free:code", "Router · Coding (default)"),
        ("free:auto", "Router · Automatic"),
        ("free:fast", "Router · Fast"),
        ("free:smart", "Router · Smart"),
        ("free:reasoning", "Router · Reasoning"),
        ("openrouter/free", "Router · OpenRouter free"),
    ]
    .into_iter()
    .map(|(id, name)| (id.into(), name.into()))
    .collect()
}

pub fn kono_models() -> Vec<(String, String)> {
    [
        ("free:tools", "Kono · Tools & Coding (default)"),
        ("free:code", "Kono · Coding"),
        ("free:fast", "Kono · Fast"),
        ("free:auto", "Kono · Automatic"),
        ("free:smart", "Kono · Smart"),
        ("free:reasoning", "Kono · Reasoning"),
    ]
    .into_iter()
    .map(|(id, name)| (id.into(), name.into()))
    .collect()
}

/// Models offered before the local server has advertised its own list.
pub fn local_models() -> Vec<(String, String)> {
    vec![(LOCAL_MODEL.into(), "Local ONNX (this machine)".into())]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_kono_and_llama_cpp_urls_without_relaxing_router_auth() {
        for (input, expected) in [
            (KONO_ORIGIN, KONO_ENDPOINT),
            ("https://kono-router.onrender.com/v1", KONO_ENDPOINT),
            ("http://127.0.0.1:8080/", "http://127.0.0.1:8080/v1"),
            ("http://[::1]:8080", "http://[::1]:8080/v1"),
            ("http://localhost:8080/v1/chat/completions", "http://localhost:8080/v1"),
            // Other hosts are OpenAI-compatible roots too: a LAN llama.cpp or
            // Ollama box and a bare HTTPS provider both need `/v1`.
            ("http://192.168.1.20:8080", "http://192.168.1.20:8080/v1"),
            ("https://api.example.com", "https://api.example.com/v1"),
            ("https://example.com/proxy", "https://example.com/proxy"),
            ("http://localhost:11434/api", "http://localhost:11434/api"),
        ] {
            assert_eq!(custom_endpoint(input), expected);
        }
        let options = AgentConfig { api_key_env: ROUTER_ENV.into(), ..Default::default() };
        assert!(authorization(&options, KONO_ENDPOINT).is_err());
    }

    #[test]
    fn router_auth_is_strictly_origin_bound() {
        for url in [
            "https://nokey-router.ziggyware.deno.net.evil/v1",
            "http://nokey-router.ziggyware.deno.net/v1",
            "https://evil/v1",
            "https://nokey-router.ziggyware.deno.net/v1?next=evil",
            "https://key@nokey-router.ziggyware.deno.net/v1",
        ] {
            assert!(!pinned_router(url));
        }
        assert!(pinned_router(ROUTER_ENDPOINT));
        assert!(pinned_router(ROUTER_ORIGIN));
        let c = AgentConfig {
            api_key_env: ROUTER_ENV.into(),
            ..Default::default()
        };
        assert!(
            authorization(&c, "https://evil/v1")
                .unwrap_err()
                .contains("Refusing")
        );
    }
    #[test]
    fn local_endpoint_resolves_default_env_and_explicit_values() {
        // SAFETY: single test in this binary touching these variables.
        unsafe {
            std::env::remove_var("TERMED_LOCAL_ENDPOINT");
            std::env::remove_var("TERMED_ONNX_ENDPOINT");
        }
        let options = AgentConfig {
            provider: "local".into(),
            ..Default::default()
        };
        assert_eq!(local_endpoint(&options), LOCAL_ENDPOINT);
        assert_eq!(endpoint(&options), LOCAL_ENDPOINT);
        assert!(local_models().iter().any(|(id, _)| id == LOCAL_MODEL));

        let explicit = AgentConfig {
            provider: "local".into(),
            local_endpoint: "http://127.0.0.1:9090".into(),
            ..Default::default()
        };
        assert_eq!(local_endpoint(&explicit), "http://127.0.0.1:9090/v1");

        unsafe {
            std::env::set_var("TERMED_LOCAL_ENDPOINT", "http://localhost:7000/v1");
        }
        assert_eq!(local_endpoint(&options), "http://localhost:7000/v1");
        unsafe {
            std::env::remove_var("TERMED_LOCAL_ENDPOINT");
        }
    }

    #[test]
    fn local_provider_never_sends_credentials() {
        let options = AgentConfig {
            provider: "local".into(),
            api_key_env: ROUTER_ENV.into(),
            ..Default::default()
        };
        // Even with the router key named, local inference gets no header.
        assert!(authorization(&options, LOCAL_ENDPOINT).unwrap().is_none());
        // And a local selection must not be mistaken for the router.
        assert!(!options.uses_router());
        assert!(options.uses_local());
    }

    #[test]
    fn router_selection_has_sane_aliases_without_breaking_custom_endpoints() {
        let mut c = AgentConfig::default();
        c.use_router();
        assert_eq!(endpoint(&c), ROUTER_ENDPOINT);
        assert_eq!(c.model, ROUTER_MODEL);
        assert_eq!(c.backend, "builtin");
        assert!(router_models().iter().any(|(id, _)| id == "free:code"));
        let c = AgentConfig {
            provider: "custom".into(),
            endpoint: "http://example.invalid/v1/".into(),
            ..Default::default()
        };
        assert_eq!(endpoint(&c), "http://example.invalid/v1");
    }
}
