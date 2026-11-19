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
pub const KONO_MODEL: &str = "free:auto";

/// Accept the root URLs users see in the Kono and llama.cpp launch output.
/// Other custom URLs may have a deliberately unversioned API or a proxy path.
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
        let (host, port) = if let Some(rest) = authority.strip_prefix("[::1]") {
            ("[::1]", rest)
        } else {
            let host = authority.split(':').next().unwrap_or("");
            (host, &authority[host.len()..])
        };
        if matches!(host, "localhost" | "127.0.0.1" | "[::1]")
            && (port.is_empty() || port.strip_prefix(':').is_some_and(|p| p.parse::<u16>().is_ok()))
        {
            return format!("{base}/v1");
        }
    }
    base.into()
}

pub fn endpoint(options: &AgentConfig) -> String {
    if options.uses_router() {
        ROUTER_ENDPOINT.into()
    } else if options.endpoint.is_empty() {
        std::env::var("TERMED_FREE_GATEWAY")
            .unwrap_or_else(|_| crate::builtin_agent::GATEWAY.into())
    } else {
        custom_endpoint(&options.endpoint)
    }
}

pub fn authorization(options: &AgentConfig, endpoint: &str) -> Result<Option<Secret>, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_kono_and_llama_cpp_urls_without_relaxing_router_auth() {
        for (input, expected) in [
            (KONO_ORIGIN, KONO_ENDPOINT),
            ("http://127.0.0.1:8080/", "http://127.0.0.1:8080/v1"),
            ("http://[::1]:8080", "http://[::1]:8080/v1"),
            ("http://localhost:8080/v1/chat/completions", "http://localhost:8080/v1"),
            ("https://example.com/proxy", "https://example.com/proxy"),
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
