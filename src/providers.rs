//! Model providers for the built-in agent, plus the small config file that
//! remembers API keys and the chosen agent.
//!
//! Every provider is plain HTTPS from Rust, so all of them work on desktop
//! and in Termux. Free ones come first: Kilo Gateway needs no account at
//! all, Ollama runs models on your own device, and several others have a
//! free tier that only needs a key.

use std::path::PathBuf;

use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    /// OpenAI-compatible `/chat/completions` (most providers).
    OpenAi,
    /// Anthropic's native Messages API.
    Anthropic,
}

#[derive(Debug)]
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    pub base: &'static str,
    /// Environment variable that overrides `base`.
    pub base_env: &'static str,
    pub api: Api,
    /// Environment variables checked for an API key, in order.
    pub key_envs: &'static [&'static str],
    pub needs_key: bool,
    /// One-line description shown in the agent picker.
    pub note: &'static str,
    /// Where to get a key.
    pub key_url: &'static str,
    /// Model ids tried first (by substring), then `default_model`.
    pub preferred: &'static [&'static str],
    pub default_model: &'static str,
    /// Free models can be swapped automatically when one is rate-limited.
    pub free: bool,
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "kilo",
        name: "Kilo Gateway",
        base: "https://api.kilo.ai/api/gateway",
        base_env: "TERMED_FREE_GATEWAY",
        api: Api::OpenAi,
        key_envs: &["KILO_API_KEY"],
        needs_key: false,
        note: "free, no account or key",
        key_url: "https://kilo.ai",
        preferred: &[],
        default_model: "kilo-auto/free",
        free: true,
    },
    Provider {
        id: "ollama",
        name: "Ollama",
        base: "http://127.0.0.1:11434/v1",
        base_env: "OLLAMA_BASE_URL",
        api: Api::OpenAi,
        key_envs: &[],
        needs_key: false,
        note: "free, private: runs on this device",
        key_url: "https://ollama.com",
        preferred: &["coder", "qwen", "llama", "gemma"],
        default_model: "",
        free: false,
    },
    Provider {
        id: "openrouter",
        name: "OpenRouter",
        base: "https://openrouter.ai/api/v1",
        base_env: "",
        api: Api::OpenAi,
        key_envs: &["OPENROUTER_API_KEY"],
        needs_key: true,
        note: "free models with a free key",
        key_url: "https://openrouter.ai/keys",
        preferred: &[],
        default_model: "openrouter/free",
        free: true,
    },
    Provider {
        id: "gemini",
        name: "Google Gemini",
        base: "https://generativelanguage.googleapis.com/v1beta/openai",
        base_env: "",
        api: Api::OpenAi,
        key_envs: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        needs_key: true,
        note: "free tier with a key",
        key_url: "https://aistudio.google.com/apikey",
        preferred: &["flash"],
        default_model: "gemini-2.5-flash",
        free: false,
    },
    Provider {
        id: "groq",
        name: "Groq",
        base: "https://api.groq.com/openai/v1",
        base_env: "",
        api: Api::OpenAi,
        key_envs: &["GROQ_API_KEY"],
        needs_key: true,
        note: "free tier with a key, very fast",
        key_url: "https://console.groq.com/keys",
        preferred: &["gpt-oss-120b", "kimi-k2", "qwen"],
        default_model: "openai/gpt-oss-120b",
        free: false,
    },
    Provider {
        id: "cerebras",
        name: "Cerebras",
        base: "https://api.cerebras.ai/v1",
        base_env: "",
        api: Api::OpenAi,
        key_envs: &["CEREBRAS_API_KEY"],
        needs_key: true,
        note: "free tier with a key, very fast",
        key_url: "https://cloud.cerebras.ai",
        preferred: &["gpt-oss-120b", "qwen-3-coder", "qwen"],
        default_model: "gpt-oss-120b",
        free: false,
    },
    Provider {
        id: "mistral",
        name: "Mistral",
        base: "https://api.mistral.ai/v1",
        base_env: "",
        api: Api::OpenAi,
        key_envs: &["MISTRAL_API_KEY"],
        needs_key: true,
        note: "free plan with a key",
        key_url: "https://console.mistral.ai/api-keys",
        preferred: &["devstral-medium", "devstral", "mistral-medium", "codestral"],
        default_model: "",
        free: false,
    },
    Provider {
        id: "anthropic",
        name: "Anthropic Claude",
        base: "https://api.anthropic.com",
        base_env: "TERMED_ANTHROPIC_BASE_URL",
        api: Api::Anthropic,
        key_envs: &["ANTHROPIC_API_KEY"],
        needs_key: true,
        note: "Claude Opus 5.5, API key",
        key_url: "https://platform.claude.com/settings/keys",
        preferred: &[],
        default_model: "claude-opus-5-5",
        free: false,
    },
    Provider {
        id: "openai",
        name: "OpenAI",
        base: "https://api.openai.com/v1",
        base_env: "",
        api: Api::OpenAi,
        key_envs: &["OPENAI_API_KEY"],
        needs_key: true,
        note: "paid API key",
        key_url: "https://platform.openai.com/api-keys",
        preferred: &["gpt-5"],
        default_model: "gpt-5",
        free: false,
    },
    Provider {
        id: "deepseek",
        name: "DeepSeek",
        base: "https://api.deepseek.com",
        base_env: "",
        api: Api::OpenAi,
        key_envs: &["DEEPSEEK_API_KEY"],
        needs_key: true,
        note: "paid API key, low cost",
        key_url: "https://platform.deepseek.com/api_keys",
        preferred: &["deepseek-chat"],
        default_model: "deepseek-chat",
        free: false,
    },
    Provider {
        id: "xai",
        name: "xAI Grok",
        base: "https://api.x.ai/v1",
        base_env: "",
        api: Api::OpenAi,
        key_envs: &["XAI_API_KEY"],
        needs_key: true,
        note: "paid API key",
        key_url: "https://console.x.ai",
        preferred: &["grok-code", "grok-4"],
        default_model: "grok-4",
        free: false,
    },
    Provider {
        id: "custom",
        name: "Custom (OpenAI-compatible)",
        base: "",
        base_env: "TERMED_OPENAI_BASE_URL",
        api: Api::OpenAi,
        key_envs: &["TERMED_OPENAI_API_KEY"],
        needs_key: false,
        note: "your OpenAI-compatible server",
        key_url: "",
        preferred: &[],
        default_model: "",
        free: false,
    },
];

pub fn get(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

impl Provider {
    pub fn base_url(&self) -> String {
        let env = (!self.base_env.is_empty())
            .then(|| std::env::var(self.base_env).ok())
            .flatten();
        let mut base = env.unwrap_or_else(|| self.base.to_string());
        // Ollama's own OLLAMA_HOST (host:port or URL) also works.
        if self.id == "ollama"
            && std::env::var(self.base_env).is_err()
            && let Ok(host) = std::env::var("OLLAMA_HOST")
        {
            let host = if host.starts_with("http") {
                host
            } else {
                format!("http://{host}")
            };
            base = format!("{}/v1", host.trim_end_matches('/'));
        }
        base.trim_end_matches('/').to_string()
    }

    /// Hidden from the picker when it can't work (custom without a URL).
    pub fn configured(&self) -> bool {
        self.id != "custom" || !self.base_url().is_empty()
    }

    pub fn api_key(&self) -> Option<String> {
        self.key_envs
            .iter()
            .find_map(|e| std::env::var(e).ok().filter(|v| !v.trim().is_empty()))
            .or_else(|| saved_key(self.id))
    }

    pub fn ready(&self) -> bool {
        !self.needs_key || self.api_key().is_some()
    }

    /// Pick a sensible model from what the provider offers.
    pub fn pick_model(&self, available: &[(String, String)]) -> String {
        // The tested default when the provider offers it, then the preferred
        // families (newest id first), then whatever comes first.
        if !self.default_model.is_empty()
            && (available.is_empty() || available.iter().any(|(id, _)| id == self.default_model))
        {
            return self.default_model.to_string();
        }
        for want in self.preferred {
            if let Some((id, _)) = available
                .iter()
                .filter(|(id, _)| id.contains(want))
                .max_by(|a, b| a.0.cmp(&b.0))
            {
                return id.clone();
            }
        }
        available
            .first()
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| self.default_model.to_string())
    }
}

/// Whether a listed model id is a chat model worth offering (OpenAI lists
/// embeddings, audio and image models too).
pub fn is_chat_model(id: &str) -> bool {
    const SKIP: &[&str] = &[
        "embed",
        "tts",
        "whisper",
        "dall-e",
        "audio",
        "realtime",
        "image",
        "moderation",
        "transcribe",
        "search",
        "davinci",
        "babbage",
        "guard",
        "rerank",
        "vision-preview",
        "-ocr",
        "lyria",
    ];
    let l = id.to_lowercase();
    !SKIP.iter().any(|s| l.contains(s))
}

// ------------------------------------------------------------ config file

/// `$TERMED_CONFIG_DIR` or `~/.config/termed`.
pub fn config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("TERMED_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("termed")
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

fn load() -> Value {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

fn store(v: &Value) -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir())?;
    let path = config_path();
    let text = serde_json::to_string_pretty(v).unwrap_or_default();
    // Keys are secrets: create the file readable by the owner only.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        f.write_all(text.as_bytes())?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, text)
    }
}

pub fn saved_key(provider: &str) -> Option<String> {
    load()
        .pointer(&format!("/keys/{provider}"))
        .and_then(Value::as_str)
        .map(String::from)
}

pub fn save_key(provider: &str, key: &str) -> std::io::Result<()> {
    let mut v = load();
    if !v.get("keys").is_some_and(Value::is_object) {
        v["keys"] = json!({});
    }
    v["keys"][provider] = json!(key.trim());
    store(&v)
}

/// The agent the user picked last (an agent name or `builtin:<provider>`).
pub fn saved_agent() -> Option<String> {
    load()
        .get("agent")
        .and_then(Value::as_str)
        .map(String::from)
}

pub fn save_agent(id: &str) -> std::io::Result<()> {
    let mut v = load();
    v["agent"] = json!(id);
    store(&v)
}

/// Pull a readable message out of an error body (OpenAI, Anthropic and
/// Google's array-wrapped format).
pub fn error_message(body: &str, status: u16) -> String {
    let v: Option<Value> = serde_json::from_str(body).ok();
    let v = match v {
        Some(Value::Array(mut a)) if !a.is_empty() => Some(a.remove(0)),
        other => other,
    };
    v.as_ref()
        .and_then(|v| {
            v.pointer("/error/message")
                .or(v.get("message"))
                .and_then(Value::as_str)
        })
        .map(String::from)
        .unwrap_or_else(|| format!("HTTP {status}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_preferred_models_and_falls_back() {
        let groq = get("groq").unwrap();
        let models = vec![
            ("llama-3.3-70b".to_string(), String::new()),
            ("openai/gpt-oss-120b".to_string(), String::new()),
        ];
        assert_eq!(groq.pick_model(&models), "openai/gpt-oss-120b");
        let ollama = get("ollama").unwrap();
        assert_eq!(
            ollama.pick_model(&[("mistral:7b".into(), String::new())]),
            "mistral:7b"
        );
        assert_eq!(get("anthropic").unwrap().pick_model(&[]), "claude-opus-5-5");
        let gemini = get("gemini").unwrap();
        let g = |ids: &[&str]| {
            ids.iter()
                .map(|i| (i.to_string(), String::new()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            gemini.pick_model(&g(&[
                "gemini-2.0-flash",
                "gemini-2.5-flash",
                "gemini-2.5-pro"
            ])),
            "gemini-2.5-flash"
        );
        let mistral = get("mistral").unwrap();
        assert_eq!(
            mistral.pick_model(&g(&[
                "devstral-small-2505",
                "devstral-medium-2507",
                "mistral-large"
            ])),
            "devstral-medium-2507"
        );
        assert!(!is_chat_model("text-embedding-3-large"));
        assert!(is_chat_model("gpt-5"));
    }

    #[test]
    fn error_messages_from_all_formats() {
        assert_eq!(
            error_message(r#"{"error":{"message":"bad key"}}"#, 401),
            "bad key"
        );
        assert_eq!(
            error_message(
                r#"[{"error":{"code":400,"message":"Please pass a valid API key"}}]"#,
                400
            ),
            "Please pass a valid API key"
        );
        assert_eq!(error_message("<html>", 502), "HTTP 502");
    }
}
