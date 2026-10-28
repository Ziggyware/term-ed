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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// auto, builtin, custom, or the display name of a detected ACP agent.
    pub backend: String,
    pub command: String,
    /// Empty strings preserve the environment / provider default.
    pub model: String,
    /// auto (legacy Kilo/custom), router, kilo, or custom. Never contains a key.
    pub provider: String,
    pub endpoint: String,
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
    pub max_output_tokens: usize,
    pub auto_context: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            backend: "builtin".into(),
            command: String::new(),
            model: String::new(),
            provider: "router".into(),
            endpoint: String::new(),
            api_key_env: String::new(),
            instructions: String::new(),
            inherited_instructions: Vec::new(),
            max_steps: 30,
            read_only: false,
            commands: CommandPolicy::Ask,
            plugins: true,
            allowed_plugins: Vec::new(),
            denied_plugins: Vec::new(),
            skills: Vec::new(),
            sub_agents: true,
            max_delegations: 4,
            task_timeout_secs: 300,
            max_agent_depth: 2,
            max_concurrent_tasks: 4,
            stall_timeout_secs: 150,
            generated_tools: true,
            max_output_tokens: 4096,
            auto_context: true,
        }
    }
}

impl AgentConfig {
    pub fn uses_router(&self) -> bool {
        self.provider == "router" || crate::provider::pinned_router(&self.endpoint)
    }

    pub fn use_router(&mut self) {
        self.backend = "builtin".into();
        self.provider = "router".into();
        self.endpoint = String::new();
        self.api_key_env = String::new();
        self.model = crate::provider::ROUTER_MODEL.into();
    }

    pub fn validate(&self) -> Result<()> {
        if !matches!(
            self.provider.as_str(),
            "auto" | "router" | "kilo" | "custom"
        ) {
            bail!("Provider must be auto, router, kilo or custom");
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
        if self.provider == "router"
            && !self.endpoint.is_empty()
            && !crate::provider::pinned_router(&self.endpoint)
        {
            bail!(
                "The router provider only accepts its pinned HTTPS endpoint; choose custom for another server"
            );
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
        if !(1..=8).contains(&self.max_concurrent_tasks) {
            bail!("Concurrent task limit must be 1–8");
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
        if !(1..=100).contains(&self.max_steps) {
            bail!("Max steps must be between 1 and 100");
        }
        if !(1..=32).contains(&self.max_delegations) {
            bail!("Delegation limit must be between 1 and 32");
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
        if !self.endpoint.is_empty()
            && !(self.endpoint.starts_with("https://") || self.endpoint.starts_with("http://"))
        {
            bail!("The API endpoint must start with http:// or https://");
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
        child.commands = if child.read_only || parent.commands == CommandPolicy::Deny {
            CommandPolicy::Deny
        } else if parent.commands == CommandPolicy::Ask && child.commands != CommandPolicy::Deny {
            CommandPolicy::Ask
        } else {
            child.commands
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
                max_steps: 12,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub version: u32,
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
        prefs.version = 2;
        prefs.validate()?;
        Ok(prefs)
    }

    /// Save to the user configuration directory, regardless of the workspace.
    /// The project argument is retained for existing callers.
    pub fn save(&self, _project: &Path) -> Result<()> {
        self.validate()?;
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
            serde_json::to_writer_pretty(&mut file, self).context("Cannot write user settings")?;
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
