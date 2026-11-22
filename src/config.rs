//! Persistent, validated user preferences, with a legacy workspace fallback.
//! Secrets are referenced by environment variable name, not stored here.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandPolicy {
    #[default]
    Ask,
    Deny,
    RiskAssessed,
    /// Never ask: run every command the risk policy does not block. Chosen
    /// with **Never ask** on a permission card or in the agent settings.
    Allow,
}

impl CommandPolicy {
    /// How much freedom a policy grants, used to keep a delegated agent from
    /// widening the permissions of the agent that delegated to it.
    pub fn rank(self) -> u8 {
        match self {
            CommandPolicy::Deny => 0,
            CommandPolicy::Ask => 1,
            CommandPolicy::RiskAssessed => 2,
            CommandPolicy::Allow => 3,
        }
    }

    /// The stricter of two policies.
    pub fn intersect(self, other: Self) -> Self {
        if self.rank() <= other.rank() {
            self
        } else {
            other
        }
    }
}

/// An active on-device ONNX inference session: the server base URL and the
/// model id it advertised.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalInference {
    pub endpoint: String,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// auto, builtin, custom, or the display name of a detected ACP agent.
    pub backend: String,
    pub command: String,
    /// Empty strings preserve the environment / provider default.
    pub model: String,
    /// The built-in provider: `router` (default), `kilo`, `custom` or `local`
    /// (on-device ONNX inference). Legacy `onnx` aliases are normalized to
    /// `local` at load and before saving; deliberate `auto` (first available
    /// backend) and other retained values are left untouched. Never contains a key.
    pub provider: String,
    pub endpoint: String,
    /// Base URL of the local ONNX inference server. Empty uses the loopback
    /// default (`http://127.0.0.1:8080/v1`); also the automatic fallback
    /// target when the remote provider returns HTTP 503.
    pub local_endpoint: String,
    pub api_key_env: String,
    pub instructions: String,
    /// Runtime-only ancestor rules; never accepted from workspace JSON.
    #[serde(skip)]
    pub inherited_instructions: Vec<String>,
    pub max_steps: usize,
    pub read_only: bool,
    pub commands: CommandPolicy,
    pub plugins: bool,
    /// Empty = all enabled plugins. Execution still needs approval every time.
    pub allowed_plugins: Vec<String>,
    pub denied_plugins: Vec<String>,
    /// Skills attached to every prompt. Other enabled skills can be read on demand.
    pub skills: Vec<String>,
    pub sub_agents: bool,
    pub max_delegations: usize,
    pub task_timeout_secs: u64,
    pub max_agent_depth: usize,
    pub max_concurrent_tasks: usize,
    pub stall_timeout_secs: u64,
    pub generated_tools: bool,
    /// Expose the extended host toolkit (workspace, text, buffer, edit,
    /// filesystem, search, git, UI, code and session tools).
    pub toolkit: bool,
    /// Toolkit groups to expose; empty means every group. Sub-agents inherit
    /// only groups their parent already has.
    pub toolkit_groups: Vec<String>,
    pub max_output_tokens: usize,
    pub auto_context: bool,
    /// Encoder model for semantic search. Empty means the built-in default;
    /// a Hugging Face repo id or a local directory both work.
    pub embedding_model: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            backend: "builtin".into(),
            command: String::new(),
            model: String::new(),
            provider: "router".into(),
            endpoint: String::new(),
            local_endpoint: String::new(),
            api_key_env: String::new(),
            instructions: String::new(),
            inherited_instructions: Vec::new(),
            max_steps: 2000,
            read_only: false,
            commands: CommandPolicy::Ask,
            plugins: true,
            allowed_plugins: Vec::new(),
            denied_plugins: Vec::new(),
            skills: Vec::new(),
            sub_agents: true,
            max_delegations: 16,
            task_timeout_secs: 300,
            max_agent_depth: 2,
            max_concurrent_tasks: 8,
            stall_timeout_secs: 150,
            generated_tools: true,
            toolkit: true,
            toolkit_groups: Vec::new(),
            max_output_tokens: 4096,
            auto_context: true,
            embedding_model: String::new(),
        }
    }
}

impl AgentConfig {
    /// A nonempty endpoint that is not the pinned router is an explicit custom
    /// server, even when the provider was left at its `router` default. Without
    /// this, a settings file that only sets the API base URL silently keeps
    /// using the NoKey Router instead of the server the user chose.
    pub fn custom_endpoint_selected(&self) -> bool {
        !self.endpoint.trim().is_empty() && !crate::provider::pinned_router(&self.endpoint)
    }

    pub fn uses_router(&self) -> bool {
        // A stale API base URL (or the pinned router URL) in settings must
        // never pull on-device inference back onto the network.
        !self.uses_local()
            && (crate::provider::pinned_router(&self.endpoint)
                || (self.provider == "router" && !self.custom_endpoint_selected()))
    }

    pub fn use_router(&mut self) {
        self.backend = "builtin".into();
        self.normalize_for_provider("router");
    }

    /// Kono Router (`https://kono-router.onrender.com/v1`): fast free cloud
    /// routing with tool support and no API key required.
    pub fn uses_kono(&self) -> bool {
        !self.uses_local()
            && (self.provider == "kono"
                || (!self.endpoint.trim().is_empty()
                    && crate::provider::custom_endpoint(&self.endpoint)
                        == crate::provider::KONO_ENDPOINT))
    }

    pub fn use_kono(&mut self) {
        self.backend = "builtin".into();
        self.normalize_for_provider("kono");
    }

    /// On-device ONNX inference: no key, no network. `onnx` is accepted as
    /// an alias for `local` in settings files.
    pub fn uses_local(&self) -> bool {
        matches!(self.provider.as_str(), "local" | "onnx")
    }

    pub fn use_local(&mut self) {
        self.backend = "builtin".into();
        self.normalize_for_provider("local");
    }

    /// The provider-independent fields that move with the provider: router
    /// credentials and pinned-router URLs never follow a switch to another
    /// server, and remote model ids never follow a switch to on-device
    /// inference. The backend is untouched here on purpose; the dedicated
    /// selectors (`use_router`, `use_local`) set it, while the settings menu
    /// leaves the user's backend untouched.
    fn normalize_for_provider(&mut self, provider: &str) {
        match provider {
            "router" => {
                self.provider = "router".into();
                self.endpoint = String::new();
                self.api_key_env = String::new();
                self.model = crate::provider::ROUTER_MODEL.into();
            }
            "kono" => {
                self.provider = "kono".into();
                self.endpoint = String::new();
                if self.api_key_env == crate::credentials::ROUTER_ENV {
                    self.api_key_env.clear();
                }
                self.model = crate::provider::KONO_MODEL.into();
            }
            "local" => {
                self.provider = "local".into();
                self.endpoint = String::new();
                if self.api_key_env == crate::credentials::ROUTER_ENV {
                    self.api_key_env.clear();
                }
                // Remote model ids mean nothing on this machine, so fall back
                // to the local server's default until the user names an ONNX model.
                let remote = self.model == crate::provider::ROUTER_MODEL
                    || self.model == crate::builtin_agent::DEFAULT_MODEL
                    || self.model == crate::provider::KONO_MODEL
                    || self.model.ends_with(":free")
                    || self.model.starts_with("free:")
                    || self.model == "openrouter/free";
                if remote {
                    self.model.clear();
                }
            }
            "kilo" => {
                // Kilo is a self-contained gateway: its gateway URL and
                // default model are used, and a router credential or router
                // model does not follow the switch. A custom URL stays so the
                // existing validation can point at it.
                self.provider = "kilo".into();
                if self.api_key_env == crate::credentials::ROUTER_ENV {
                    self.api_key_env.clear();
                }
                if self.model == crate::provider::ROUTER_MODEL {
                    self.model.clear();
                }
            }
            "custom" => {
                // Custom keeps the user's own endpoint; a router credential or
                // the pinned router URL does not follow the switch.
                self.provider = "custom".into();
                if self.api_key_env == crate::credentials::ROUTER_ENV {
                    self.api_key_env.clear();
                }
                if self.model == crate::provider::ROUTER_MODEL {
                    self.model.clear();
                }
                if crate::provider::pinned_router(self.endpoint.trim()) {
                    self.endpoint.clear();
                }
            }
            _ => unreachable!("normalize_for_provider called with an unknown provider"),
        }
    }

    /// The providers the built-in agent accepts. Deliberate legacy or
    /// inheritance values (`auto`) are retained; every other value must be
    /// one of these in the settings file.
    pub const CANONICAL_PROVIDERS: [&str; 5] = ["router", "kono", "kilo", "custom", "local"];

    /// Normalize a provider value to its canonical form: trimmed, lowercased,
    /// with the legacy `onnx` alias mapped to `local`.
    pub fn canonical_provider(provider: &str) -> String {
        let value = provider.trim().to_ascii_lowercase();
        match value.as_str() {
            "onnx" => "local".into(),
            _ => value,
        }
    }

    /// Normalize and apply a provider selection. The related fields (model,
    /// endpoint, key variable) move together with the provider, so a file can
    /// never silently keep e.g. a router credential paired with a custom
    /// endpoint or a remote model with on-device inference. The backend is
    /// deliberately NOT changed here: external ACP backends are selected with
    /// Backend, and `auto` keeps the first-available-backend meaning.
    /// Deliberately retained `auto` keeps its fields untouched.
    pub fn set_provider(&mut self, provider: &str) -> Result<()> {
        let provider = Self::canonical_provider(provider);
        match provider.as_str() {
            "auto" => {
                self.provider = "auto".into();
                Ok(())
            }
            "router" | "kono" | "local" | "kilo" | "custom" => {
                // Provider settings belong to the built-in agent. An external
                // ACP backend ignores them entirely (it keeps its own model),
                // and `auto` selects the first available backend, which the
                // router/local fallbacks do too. Only the provider itself is
                // normalized for external backends.
                if self.backend != "builtin" && self.backend != "auto" {
                    self.provider = provider;
                    return Ok(());
                }
                self.normalize_for_provider(&provider);
                Ok(())
            }
            other => bail!(
                "Unknown provider \"{other}\"; choose from {} or auto (first available backend)",
                Self::CANONICAL_PROVIDERS.join(", ")
            ),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if !matches!(
            self.provider.as_str(),
            "auto" | "router" | "kono" | "kilo" | "custom" | "local" | "onnx"
        ) {
            bail!("Provider must be router, kono, kilo, custom, local or auto");
        }
        if self.provider == "custom" && self.endpoint.trim().is_empty() {
            bail!("Set an API base URL before choosing the custom provider");
        }
        if self.provider == "kilo"
            && !self.endpoint.is_empty()
            && self.endpoint.trim_end_matches('/') != crate::builtin_agent::GATEWAY
        {
            bail!("Kilo uses its default gateway; choose auto/custom for another API base URL");
        }
        if self.uses_router()
            && !self.api_key_env.is_empty()
            && self.api_key_env != crate::credentials::ROUTER_ENV
        {
            bail!("Router credentials use ROUTER_API_KEY or the saved local key");
        }
        if self.api_key_env == crate::credentials::ROUTER_ENV && !self.uses_router() {
            bail!("ROUTER_API_KEY cannot be sent to another provider");
        }
        if !(1..=3).contains(&self.max_agent_depth) {
            bail!("Agent depth must be 1–3 (root is depth 0)");
        }
        if !(1..=32).contains(&self.max_concurrent_tasks) {
            bail!("Concurrent task limit must be 1–32");
        }
        if !(15..=600).contains(&self.stall_timeout_secs) {
            bail!("Stall timeout must be 15–600 seconds");
        }
        if !(256..=16384).contains(&self.max_output_tokens) {
            bail!("Output token budget must be 256–16384");
        }
        if self.inherited_instructions.len() > 3
            || self
                .inherited_instructions
                .iter()
                .any(|s| s.len() > 16 * 1024)
        {
            bail!("Inherited instruction budget exceeded");
        }
        if self.instructions.len() > 16 * 1024 {
            bail!("Agent instructions must not exceed 16 KiB");
        }
        for group in &self.toolkit_groups {
            if !crate::toolkit::group_ids().contains(&group.as_str()) {
                bail!(
                    "Unknown toolkit group \"{group}\"; choose from {}",
                    crate::toolkit::group_ids().join(", ")
                );
            }
        }
        if !(1..=10_000).contains(&self.max_steps) {
            bail!("Max steps must be between 1 and 10000");
        }
        if !(1..=64).contains(&self.max_delegations) {
            bail!("Delegation limit must be between 1 and 64");
        }
        if !(10..=1800).contains(&self.task_timeout_secs) {
            bail!("Task timeout must be 10–1800 seconds");
        }
        if self.backend.trim().is_empty() {
            bail!("Choose an agent backend");
        }
        if self.backend.trim().eq_ignore_ascii_case("opencode")
            || (self.backend == "custom" && crate::agent::removed_opencode_command(&self.command))
        {
            bail!(
                "OpenCode integration has been removed; choose the built-in or another ACP agent"
            );
        }
        if self.backend == "custom" && crate::plugin::split_command(&self.command).is_empty() {
            bail!("A custom ACP agent needs a command");
        }
        if !(self.endpoint.is_empty() || self.endpoint.starts_with("https://") || self.endpoint.starts_with("http://"))
        {
            bail!("The API endpoint must start with http:// or https://");
        }
        if !(self.local_endpoint.is_empty()
            || self.local_endpoint.starts_with("https://")
            || self.local_endpoint.starts_with("http://"))
        {
            bail!("The local ONNX endpoint must start with http:// or https://");
        }
        if !self.api_key_env.is_empty()
            && !self
                .api_key_env
                .chars()
                .enumerate()
                .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
        {
            bail!("API key environment variable must be a variable name, not a secret");
        }
        Ok(())
    }

    /// A delegated worker cannot widen its parent's host capabilities.
    pub fn restricted_by(&self, parent: &Self) -> Self {
        let mut child = self.clone();
        child.backend = "builtin".into();
        child.inherited_instructions = parent.inherited_instructions.clone();
        if !parent.instructions.trim().is_empty() {
            child
                .inherited_instructions
                .push(parent.instructions.clone());
        }
        child.read_only |= parent.read_only;
        child.plugins &= parent.plugins && !child.read_only;
        // A child may narrow its parent's command permissions, never widen them.
        child.commands = if child.read_only {
            CommandPolicy::Deny
        } else {
            child.commands.intersect(parent.commands)
        };
        for name in &parent.denied_plugins {
            if !child.denied_plugins.contains(name) {
                child.denied_plugins.push(name.clone());
            }
        }
        if !parent.allowed_plugins.is_empty() {
            child.allowed_plugins = if child.allowed_plugins.is_empty() {
                parent.allowed_plugins.clone()
            } else {
                child
                    .allowed_plugins
                    .into_iter()
                    .filter(|p| parent.allowed_plugins.contains(p))
                    .collect()
            };
            // An empty intersection must not turn into "all plugins".
            if child.allowed_plugins.is_empty() {
                child.plugins = false;
            }
        }
        let inherits_provider = child.provider == "auto"
            && (child.endpoint.is_empty() || child.endpoint == parent.endpoint);
        if inherits_provider {
            if child.endpoint.is_empty() {
                child.endpoint = parent.endpoint.clone();
            }
            child.provider = parent.provider.clone();
        }
        // Children inherit the local inference server too, whether they came
        // from an `auto` provider or explicitly ask for on-device inference.
        if child.uses_local() && child.local_endpoint.is_empty() {
            child.local_endpoint = parent.local_endpoint.clone();
        }
        // Do not accidentally forward a provider's credential to a different
        // explicitly configured specialist endpoint.
        if child.api_key_env.is_empty() && inherits_provider {
            child.api_key_env = parent.api_key_env.clone();
        }
        child.max_steps = child.max_steps.min(parent.max_steps);
        if child.model.is_empty() && inherits_provider {
            child.model = parent.model.clone();
        }
        child.task_timeout_secs = child.task_timeout_secs.min(parent.task_timeout_secs);
        child.sub_agents &= parent.sub_agents;
        child.generated_tools &= parent.generated_tools;
        child.toolkit &= parent.toolkit;
        // A parent that narrowed groups keeps the child inside the same set.
        if !parent.toolkit_groups.is_empty() {
            child.toolkit_groups = if child.toolkit_groups.is_empty() {
                parent.toolkit_groups.clone()
            } else {
                child
                    .toolkit_groups
                    .iter()
                    .filter(|g| parent.toolkit_groups.iter().any(|p| p == *g))
                    .cloned()
                    .collect()
            };
            if child.toolkit_groups.is_empty() {
                child.toolkit = false;
            }
        }
        child.max_agent_depth = child.max_agent_depth.min(parent.max_agent_depth);
        child.max_concurrent_tasks = child.max_concurrent_tasks.min(parent.max_concurrent_tasks);
        child.max_delegations = child.max_delegations.min(parent.max_delegations);
        child.stall_timeout_secs = child.stall_timeout_secs.min(parent.stall_timeout_secs);
        child.max_output_tokens = child.max_output_tokens.min(parent.max_output_tokens);
        // Required project skills survive every generation; each role may add skills.
        let mut inherited = parent.skills.clone();
        for skill in &child.skills {
            if !inherited.contains(skill) {
                inherited.push(skill.clone());
            }
        }
        child.skills = inherited;
        child
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubAgentConfig {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub agent: AgentConfig,
}

impl Default for SubAgentConfig {
    fn default() -> Self {
        Self {
            id: "worker".into(),
            name: "Worker".into(),
            description: String::new(),
            enabled: true,
            agent: AgentConfig {
                backend: "builtin".into(),
                // Specialists inherit the parent's provider unless explicitly set.
                provider: "auto".into(),
                read_only: true,
                max_steps: 2000,
                sub_agents: false,
                ..Default::default()
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    pub theme: String,
    pub minimap: bool,
    pub whitespace: bool,
    pub indent_guides: bool,
    pub line_numbers: bool,
    pub word_wrap: Option<bool>,
    pub toolbar: Option<bool>,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            theme: String::new(),
            minimap: true,
            whitespace: false,
            indent_guides: true,
            line_numbers: true,
            word_wrap: None,
            toolbar: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    pub max_results: usize,
    pub ignore_binary: bool,
}
impl Default for SearchConfig {
    fn default() -> Self {
        Self { max_results: crate::search::DEFAULT_RESULT_LIMIT, ignore_binary: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub version: u32,
    pub search: SearchConfig,
    pub editor: EditorConfig,
    pub agent: AgentConfig,
    pub sub_agents: Vec<SubAgentConfig>,
    pub disabled_skills: Vec<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        let reviewer = SubAgentConfig { id: "reviewer".into(), name: "Reviewer".into(),
            description: "Read-only code review and bug analysis".into(),
            agent: AgentConfig { instructions: "Review the requested code. Identify concrete bugs, security risks and missing tests; cite files and lines. Do not edit files.".into(), ..SubAgentConfig::default().agent },
            ..Default::default() };
        let tester = SubAgentConfig { id: "tester".into(), name: "Test planner".into(),
            description: "Read-only test design and coverage analysis".into(),
            agent: AgentConfig { instructions: "Inspect the code and propose focused regression tests, edge cases and verification commands. Do not edit or execute commands.".into(), ..SubAgentConfig::default().agent },
            ..Default::default() };
        let coordinator=SubAgentConfig {id:"coordinator".into(),name:"Coordinator".into(),
            description:"Read-only coordinator that can delegate focused reviews and test planning to grandchildren".into(),
            agent:AgentConfig{backend:"builtin".into(),provider:"auto".into(),read_only:true,commands:CommandPolicy::Deny,plugins:false,max_steps:18,sub_agents:true,
                instructions:"Coordinate a focused investigation. When useful, delegate distinct evidence-based tasks to reviewer and tester, passing relevant findings, scope and acceptance criteria. Collect every result before finishing, verify claims, and report disagreements, blockers and checks not run. Do not edit files or execute commands.".into(),..Default::default()},..Default::default()};
        Self {
            version: 2,
            editor: EditorConfig::default(),
            search: SearchConfig::default(),
            agent: AgentConfig::default(),
            sub_agents: vec![reviewer, tester, coordinator],
            disabled_skills: Vec::new(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    // A relative/empty XDG_CONFIG_HOME would accidentally make settings vary
    // with the current project directory. The XDG spec requires an absolute path.
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| {
            let home = ["HOME", "USERPROFILE"]
                .into_iter()
                .filter_map(std::env::var_os)
                .map(PathBuf::from)
                .find(|path| path.is_absolute());
            #[allow(deprecated)]
            let home = home.or_else(|| std::env::home_dir().filter(|path| path.is_absolute()));
            home.unwrap_or_else(std::env::temp_dir).join(".config")
        })
        .join("termed")
}

/// Old per-workspace settings, used only until user settings are saved.
pub fn settings_path(project: &Path) -> PathBuf {
    project.join(".termed/settings.json")
}

/// Settings used for every workspace, alongside (but separate from) credentials.
pub fn user_settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Preferences {
    pub fn validate(&self) -> Result<()> {
        if !matches!(self.version, 1 | 2) {
            bail!("Unsupported settings version {}", self.version);
        }
        self.agent.validate()?;
        if !(1..=crate::search::MAX_RESULT_LIMIT).contains(&self.search.max_results) {
            bail!("Search result limit must be between 1 and 100000");
        }
        if self.sub_agents.len() > 16 {
            bail!("At most 16 sub-agent profiles are supported");
        }
        let mut seen = std::collections::HashSet::new();
        for p in &self.sub_agents {
            if !valid_id(&p.id) || !seen.insert(&p.id) {
                bail!("Sub-agent IDs must be unique: {}", p.id);
            }
            if p.name.trim().is_empty() {
                bail!("Sub-agents need a display name");
            }
            p.agent
                .validate()
                .with_context(|| format!("Sub-agent {}", p.id))?;
        }
        if !self.editor.theme.is_empty() && crate::theme::load(&self.editor.theme).is_none() {
            bail!("Unknown color theme");
        }
        Ok(())
    }

    pub fn load(project: &Path) -> Result<Self> {
        let mut value = serde_json::to_value(Self::default())?;
        let mut selected_in_file = false;
        let mut legacy_schema = false;
        // Existing per-project preferences are a migration source only. Once
        // user settings exist, old workspace files cannot override or break
        // the preferences chosen for all folders.
        let user_path = user_settings_path();
        let mut source = None;
        for path in [user_path, settings_path(project)] {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    source = Some((path, text));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("Cannot read {}", path.display())),
            }
        }
        if let Some((path, text)) = source {
            let mut overlay: Value = serde_json::from_str(&text)
                .with_context(|| format!("Invalid JSON in {}", path.display()))?;
            if !overlay.is_object() {
                bail!("{} must contain a JSON object", path.display());
            }
            // Version 1 and unversioned files used the anonymous gateway as
            // the implicit built-in provider. Version 2 makes the router the
            // default while keeping a deliberately chosen `auto` provider.
            legacy_schema = overlay
                .get("version")
                .is_none_or(|version| version.as_u64() == Some(1));
            selected_in_file = overlay
                .get("agent")
                .and_then(Value::as_object)
                .is_some_and(|a| {
                    ["backend", "provider", "endpoint"]
                        .iter()
                        .any(|key| a.contains_key(*key))
                });
            prepare_legacy_overlay(&mut overlay);
            merge(&mut value, overlay);
        }
        let mut prefs: Self = serde_json::from_value(value).context("Invalid settings value")?;
        // Preserve an explicit custom ACP environment override for users who
        // have not selected a backend/provider in their settings. It cannot
        // resurrect the removed OpenCode integration.
        let mut environment_agent = false;
        if !selected_in_file
            && let Ok(command) = std::env::var("TERMED_AGENT")
            && command != "builtin"
            && !crate::plugin::split_command(&command).is_empty()
            && !crate::agent::removed_opencode_command(&command)
        {
            prefs.agent.backend = "auto".into();
            prefs.agent.provider = "auto".into();
            environment_agent = true;
        }
        if !matches!(prefs.version, 1 | 2) {
            bail!("Unsupported settings version {}", prefs.version);
        }
        migrate_removed_backend(&mut prefs.agent, !environment_agent, legacy_schema);
        for profile in &mut prefs.sub_agents {
            migrate_removed_backend(&mut profile.agent, false, legacy_schema);
        }
        canonicalize_providers(&mut prefs);
        prefs.version = 2;
        prefs.validate()?;
        Ok(prefs)
    }

    /// Save to the user configuration directory, regardless of the workspace.
    /// The project argument is retained for existing callers.
    pub fn save(&self, _project: &Path) -> Result<()> {
        let prefs = {
            let mut prefs = self.clone();
            canonicalize_providers(&mut prefs);
            prefs
        };
        prefs.validate()?;
        let path = user_settings_path();
        let parent = path.parent().context("User settings path has no parent")?;
        let mut directories = std::fs::DirBuilder::new();
        directories.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            directories.mode(0o700);
        }
        directories
            .create(parent)
            .context("Cannot create user settings directory")?;
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let tmp = parent.join(format!(
            ".settings-{}-{}.tmp",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        // Write a unique sibling and rename only after it is complete. This
        // preserves the previous file on errors and avoids cross-process clashes.
        let result = (|| -> Result<()> {
            let mut file = options.open(&tmp).context("Cannot create user settings")?;
            serde_json::to_writer_pretty(&mut file, &prefs).context("Cannot write user settings")?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp, &path).context("Cannot replace user settings file")?;
            Ok(())
        })();
        let _ = std::fs::remove_file(&tmp);
        result
    }
}

/// Older partial settings used an implicit custom endpoint and specialist
/// provider inheritance. Preserve those meanings with the new router default.
fn prepare_legacy_overlay(overlay: &mut Value) {
    if let Some(agent) = overlay.get_mut("agent").and_then(Value::as_object_mut) {
        let custom_endpoint = agent
            .get("endpoint")
            .and_then(Value::as_str)
            .is_some_and(|url| !url.is_empty() && !crate::provider::pinned_router(url));
        if custom_endpoint && !agent.contains_key("provider") {
            agent.insert("provider".into(), Value::String("auto".into()));
        }
    }
    if let Some(profiles) = overlay.get_mut("sub_agents").and_then(Value::as_array_mut) {
        for profile in profiles {
            if let Some(agent) = profile.get_mut("agent").and_then(Value::as_object_mut) {
                agent
                    .entry("provider")
                    .or_insert_with(|| Value::String("auto".into()));
            }
        }
    }
}

/// Normalize every stored provider value in place (legacy aliases never
/// persist). Deliberately retained values (`auto`) pass through untouched.
fn canonicalize_providers(prefs: &mut Preferences) {
    fn clean_agent(agent: &mut AgentConfig) {
        agent.provider = AgentConfig::canonical_provider(&agent.provider);
        if agent.uses_local() && matches!(agent.backend.as_str(), "builtin" | "auto") {
            let remote = agent.model == crate::provider::ROUTER_MODEL
                || agent.model == crate::builtin_agent::DEFAULT_MODEL
                || agent.model == crate::provider::KONO_MODEL
                || agent.model.ends_with(":free")
                || agent.model.starts_with("free:")
                || agent.model == "openrouter/free";
            if remote {
                agent.model.clear();
            }
        }
    }
    clean_agent(&mut prefs.agent);
    for profile in &mut prefs.sub_agents {
        clean_agent(&mut profile.agent);
    }
}

/// Drop the removed backend and migrate implicit legacy defaults, while
/// preserving explicit custom endpoints and deliberately selected providers.
fn migrate_removed_backend(agent: &mut AgentConfig, primary: bool, legacy_schema: bool) {
    let removed = agent.backend.trim().eq_ignore_ascii_case("opencode")
        || (agent.backend == "custom" && crate::agent::removed_opencode_command(&agent.command));
    let implicit_provider = agent.provider == "auto"
        && agent.endpoint.is_empty()
        && (agent.api_key_env.is_empty() || agent.api_key_env == crate::credentials::ROUTER_ENV);
    let legacy_auto = primary
        && legacy_schema
        && agent.backend == "auto"
        && (agent.uses_router() || implicit_provider);
    if removed || legacy_auto {
        agent.backend = "builtin".into();
        if removed {
            agent.command.clear();
        }
    }
    if primary && (legacy_schema || removed) && agent.backend == "builtin" && implicit_provider {
        agent.provider = "router".into();
    }
    let model = agent.model.to_ascii_lowercase();
    if (removed || legacy_auto || (primary && agent.uses_router()))
        && (model.starts_with("opencode/") || model.ends_with("big-pickle"))
    {
        agent.model.clear();
    }
}

fn merge(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                merge(b.entry(k).or_insert(Value::Null), v);
            }
        }
        (b, o) => *b = o,
    }
}

/// Resolve a workspace path without allowing `..` or existing symlinks to
/// escape the root. Also checks parents of paths that have not been created.
pub fn workspace_path(root: &Path, input: &str) -> Result<PathBuf, String> {
    let root = crate::search::absolute_path(root);
    let input = Path::new(input);
    let path = crate::search::absolute_path(&if input.is_absolute() {
        input.to_path_buf()
    } else {
        root.join(input)
    });
    if !path.starts_with(&root) {
        return Err("Path is outside the workspace".into());
    }
    let mut ancestor = path.as_path();
    loop {
        if let Ok(real) = ancestor.canonicalize() {
            if !real.starts_with(&root) {
                return Err("Path follows a symlink outside the workspace".into());
            }
            break;
        }
        let Some(parent) = ancestor.parent() else {
            break;
        };
        ancestor = parent;
    }
    if crate::credentials::is_private_path(&path) {
        return Err(
            "Local credential files are private and unavailable to agent file/context tools".into(),
        );
    }
    Ok(path)
}

#[cfg(test)]
mod command_policy_tests {
    use super::*;

    #[test]
    fn canonical_provider_maps_aliases_and_normalizes() {
        assert_eq!(AgentConfig::canonical_provider("onnx"), "local");
        assert_eq!(AgentConfig::canonical_provider(" ONNX "), "local");
        assert_eq!(AgentConfig::canonical_provider("local"), "local");
        assert_eq!(AgentConfig::canonical_provider(" Router "), "router");
        assert_eq!(AgentConfig::canonical_provider("custom"), "custom");
    }

    #[test]
    fn set_provider_normalizes_the_related_fields() {
        let mut router_first = AgentConfig {
            endpoint: String::new(),
            api_key_env: crate::credentials::ROUTER_ENV.into(),
            model: crate::provider::ROUTER_MODEL.into(),
            ..Default::default()
        };
        router_first.set_provider("local").unwrap();
        assert_eq!(router_first.provider, "local");
        assert_eq!(router_first.backend, "builtin");
        assert!(router_first.endpoint.is_empty());
        assert!(router_first.api_key_env.is_empty());
        assert!(router_first.model.is_empty());
        router_first.validate().unwrap();

        let mut custom = AgentConfig {
            api_key_env: crate::credentials::ROUTER_ENV.into(),
            model: crate::provider::ROUTER_MODEL.into(),
            endpoint: crate::provider::ROUTER_ENDPOINT.into(),
            ..Default::default()
        };
        custom.set_provider("custom").unwrap();
        assert_eq!(custom.provider, "custom");
        assert!(custom.api_key_env.is_empty());
        assert!(custom.model.is_empty());
        assert!(custom.endpoint.is_empty());

        let mut kilo = AgentConfig {
            api_key_env: crate::credentials::ROUTER_ENV.into(),
            model: crate::provider::ROUTER_MODEL.into(),
            ..Default::default()
        };
        kilo.set_provider("kilo").unwrap();
        assert_eq!(kilo.provider, "kilo");
        assert!(kilo.api_key_env.is_empty());
        assert!(kilo.model.is_empty());

        // A custom URL survives switching to kilo; the existing validation
        // then does its job.
        let mut kilo_with_url = kilo.clone();
        kilo_with_url.endpoint = "https://one.example.com/v1".into();
        kilo_with_url.set_provider("kilo").unwrap();
        assert_eq!(kilo_with_url.endpoint, "https://one.example.com/v1");

        let mut local_again = AgentConfig {
            model: "onnx-community/SmolLM2-135M-Instruct".into(),
            ..Default::default()
        };
        local_again.use_local();
        local_again.set_provider("onnx").unwrap();
        assert_eq!(local_again.provider, "local");
        assert_eq!(local_again.model, "onnx-community/SmolLM2-135M-Instruct");

        assert!(AgentConfig::default().set_provider("bogus").is_err());
    }

    #[test]
    fn provider_edits_leave_external_backends_alone() {
        // Provider settings belong to the built-in agent; an external ACP
        // backend must keep its own model, endpoint and backend untouched.
        let mut external = AgentConfig {
            backend: "external-agent".into(),
            model: "some-model".into(),
            endpoint: "https://ext.example.com/v1".into(),
            ..Default::default()
        };
        external.set_provider("local").unwrap();
        assert_eq!(external.backend, "external-agent");
        assert_eq!(external.provider, "local");
        assert_eq!(external.model, "some-model");
        assert_eq!(external.endpoint, "https://ext.example.com/v1");
    }

    #[test]
    fn canonicalize_providers_drops_aliases_and_keeps_deliberate_auto() {
        let mut prefs = Preferences {
            agent: AgentConfig {
                provider: "ONNX".into(),
                ..Default::default()
            },
            sub_agents: vec![SubAgentConfig {
                agent: AgentConfig {
                    provider: "onnx".into(),
                    ..Default::default()
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        canonicalize_providers(&mut prefs);
        assert_eq!(prefs.agent.provider, "local");
        assert_eq!(prefs.sub_agents[0].agent.provider, "local");

        let mut deliberate = Preferences::default();
        deliberate.agent.provider = "auto".into();
        canonicalize_providers(&mut deliberate);
        assert_eq!(deliberate.agent.provider, "auto");
    }

    #[test]
    fn never_ask_round_trips_through_settings_json() {
        let agent = AgentConfig {
            commands: CommandPolicy::Allow,
            ..Default::default()
        };
        let text = serde_json::to_string(&agent).unwrap();
        assert!(text.contains("\"commands\":\"allow\""), "{text}");
        let back: AgentConfig = serde_json::from_str(&text).unwrap();
        assert_eq!(back.commands, CommandPolicy::Allow);
    }

    #[test]
    fn explicit_endpoint_overrides_the_default_router_provider() {
        // Setting only the API base URL (the documented Kono/llama.cpp flow)
        // must not be ignored or rejected because `provider` kept its default.
        let kono = AgentConfig {
            endpoint: "https://kono-router.onrender.com".into(),
            ..Default::default()
        };
        assert_eq!(kono.provider, "router");
        assert!(!kono.uses_router());
        assert_eq!(
            crate::provider::endpoint(&kono),
            crate::provider::KONO_ENDPOINT
        );
        kono.validate().unwrap();

        let lan = AgentConfig {
            endpoint: "http://192.168.1.20:8080".into(),
            ..Default::default()
        };
        assert!(!lan.uses_router());
        assert_eq!(
            crate::provider::endpoint(&lan),
            "http://192.168.1.20:8080/v1"
        );
        lan.validate().unwrap();

        // The pinned endpoint still selects the router regardless of provider.
        let pinned = AgentConfig {
            provider: "custom".into(),
            endpoint: crate::provider::ROUTER_ORIGIN.into(),
            ..Default::default()
        };
        assert!(pinned.uses_router());
    }

    #[test]
    fn local_provider_validates_and_clears_router_defaults() {
        let mut local = AgentConfig::default();
        local.use_local();
        assert!(local.uses_local());
        assert!(!local.uses_router());
        assert_eq!(local.provider, "local");
        assert_eq!(local.backend, "builtin");
        local.validate().unwrap();
        assert_eq!(
            crate::provider::endpoint(&local),
            crate::provider::LOCAL_ENDPOINT
        );

        // The `onnx` alias is accepted too.
        let alias = AgentConfig {
            provider: "onnx".into(),
            ..Default::default()
        };
        assert!(alias.uses_local());
        alias.validate().unwrap();

        // The router key must never be paired with local inference.
        let bad = AgentConfig {
            provider: "local".into(),
            api_key_env: crate::credentials::ROUTER_ENV.into(),
            ..Default::default()
        };
        assert!(bad.validate().is_err());

        // A malformed local endpoint is rejected.
        let bad_url = AgentConfig {
            provider: "local".into(),
            local_endpoint: "localhost:8080".into(),
            ..Default::default()
        };
        assert!(bad_url.validate().is_err());
    }

    #[test]
    fn delegated_agents_inherit_the_local_endpoint() {
        let parent = AgentConfig {
            provider: "local".into(),
            local_endpoint: "http://127.0.0.1:9090/v1".into(),
            ..Default::default()
        };
        let child = AgentConfig {
            provider: "auto".into(),
            ..Default::default()
        };
        let restricted = child.restricted_by(&parent);
        assert!(restricted.uses_local());
        assert_eq!(restricted.local_endpoint, "http://127.0.0.1:9090/v1");

        // An explicit local child inherits the endpoint without the provider.
        let local_child = AgentConfig {
            provider: "local".into(),
            ..Default::default()
        };
        let restricted = local_child.restricted_by(&parent);
        assert!(restricted.uses_local());
        assert_eq!(restricted.local_endpoint, "http://127.0.0.1:9090/v1");
    }

    #[test]
    fn delegated_agents_cannot_widen_command_permissions() {
        let child = AgentConfig {
            commands: CommandPolicy::Allow,
            ..Default::default()
        };
        for (parent, expected) in [
            (CommandPolicy::Ask, CommandPolicy::Ask),
            (CommandPolicy::Deny, CommandPolicy::Deny),
            (CommandPolicy::RiskAssessed, CommandPolicy::RiskAssessed),
            (CommandPolicy::Allow, CommandPolicy::Allow),
        ] {
            let parent = AgentConfig {
                commands: parent,
                ..Default::default()
            };
            assert_eq!(child.restricted_by(&parent).commands, expected);
        }
        // A stricter child stays strict.
        let strict = AgentConfig {
            commands: CommandPolicy::Deny,
            ..Default::default()
        };
        let parent = AgentConfig {
            commands: CommandPolicy::Allow,
            ..Default::default()
        };
        assert_eq!(strict.restricted_by(&parent).commands, CommandPolicy::Deny);
    }
}