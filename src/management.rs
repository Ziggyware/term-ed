//! Keyboard/touch-accessible settings, plugin, sub-agent and skill menus.
//! Menus reuse the editor's searchable palette rather than introducing a
//! second navigation/input system.
use crate::app::{App, PaletteMode, ToastKind};
use crate::config::{self, AgentConfig, CommandPolicy, Preferences, SubAgentConfig};
use anyhow::{Result, bail};

#[derive(Default)]
pub struct Management {
    pub plugin: Option<usize>,
    pub sub_agent: Option<String>,
    pub skill: Option<String>,
    pub edit: Option<Edit>,
    pub load_error: Option<String>,
}

#[derive(Clone)]
pub struct Edit {
    pub title: String,
    pub hint: String,
    pub value: String,
    pub secret: bool,
    target: EditTarget,
}

#[derive(Clone)]
enum EditTarget {
    Main(Field),
    Sub(String, Field),
    SubName(String),
    SubDescription(String),
    CreateSub,
    CreateSkill,
    RouterKey,
}

#[derive(Clone, Copy)]
enum Field {
    Backend,
    Command,
    Model,
    Endpoint,
    ApiKeyEnv,
    Instructions,
    MaxSteps,
    ReadOnly,
    Commands,
    Plugins,
    AllowedPlugins,
    Skills,
    SubAgents,
    MaxDelegations,
    Timeout,
    AutoContext,
    Provider,
    MaxDepth,
    Concurrency,
    StallTimeout,
    GeneratedTools,
    OutputTokens,
}

const FIELDS: &[(Field, &str)] = &[
    (Field::Backend, "Backend"),
    (Field::Command, "Custom ACP command"),
    (Field::Model, "Model"),
    (Field::Endpoint, "API base URL"),
    (Field::ApiKeyEnv, "API key environment variable"),
    (Field::Instructions, "Instructions"),
    (Field::MaxSteps, "Step budget"),
    (Field::ReadOnly, "Read-only"),
    (Field::Commands, "Command permissions"),
    (Field::Plugins, "AI plugin tools"),
    (Field::AllowedPlugins, "Plugin allowlist"),
    (Field::Skills, "Attached skills"),
    (Field::SubAgents, "Sub-agent delegation"),
    (Field::MaxDelegations, "Delegations per turn"),
    (Field::Timeout, "Task timeout (seconds)"),
    (Field::AutoContext, "Attach editor context"),
    (Field::Provider, "Built-in provider"),
    (Field::MaxDepth, "Maximum agent depth"),
    (Field::Concurrency, "Concurrent tasks"),
    (Field::StallTimeout, "Overwatch stall timeout (seconds)"),
    (Field::GeneratedTools, "Pure generated functions"),
    (Field::OutputTokens, "Model output token budget"),
];

fn on(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}
fn auto(b: Option<bool>) -> &'static str {
    match b {
        None => "automatic",
        Some(true) => "on",
        Some(false) => "off",
    }
}
fn cycle(b: Option<bool>) -> Option<bool> {
    match b {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
    }
}
fn csv(s: &str) -> Vec<String> {
    s.split([',', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}
fn value(field: Field, a: &AgentConfig) -> String {
    match field {
        Field::Backend => a.backend.clone(),
        Field::Command => a.command.clone(),
        Field::Model => a.model.clone(),
        Field::Endpoint => a.endpoint.clone(),
        Field::ApiKeyEnv => a.api_key_env.clone(),
        Field::Instructions => a.instructions.clone(),
        Field::MaxSteps => a.max_steps.to_string(),
        Field::ReadOnly => on(a.read_only).into(),
        Field::Commands => match a.commands {
            CommandPolicy::Ask => "ask",
            CommandPolicy::Deny => "deny",
            CommandPolicy::RiskAssessed => "risk_assessed",
        }
        .into(),
        Field::Plugins => on(a.plugins).into(),
        Field::AllowedPlugins => a.allowed_plugins.join(", "),
        Field::Skills => a.skills.join(", "),
        Field::SubAgents => on(a.sub_agents).into(),
        Field::MaxDelegations => a.max_delegations.to_string(),
        Field::Timeout => a.task_timeout_secs.to_string(),
        Field::AutoContext => on(a.auto_context).into(),
        Field::Provider => a.provider.clone(),
        Field::MaxDepth => a.max_agent_depth.to_string(),
        Field::Concurrency => a.max_concurrent_tasks.to_string(),
        Field::StallTimeout => a.stall_timeout_secs.to_string(),
        Field::GeneratedTools => on(a.generated_tools).into(),
        Field::OutputTokens => a.max_output_tokens.to_string(),
    }
}

fn hint(field: Field) -> &'static str {
    match field {
        Field::Backend => {
            "auto, builtin, custom, or a detected ACP agent name; external agents must support MCP for plugin tools"
        }
        Field::Command => {
            "Quoted argv is supported, e.g. python3 '/path/my agent.py'; set this before choosing custom"
        }
        Field::Model => {
            "Empty = provider/environment default. Built-in: any tool-capable model supported by your API"
        }
        Field::Endpoint => {
            "Built-in only: OpenAI-compatible base URL (include /v1 if needed); empty = free gateway"
        }
        Field::ApiKeyEnv => {
            "Environment variable NAME only (e.g. OPENAI_API_KEY). Never paste a secret here"
        }
        Field::Instructions => {
            "Role and project conventions. Use Settings JSON for multi-line instructions"
        }
        Field::MaxSteps => {
            "Built-in model/tool rounds, 1–100; external ACP agents control their own budgets"
        }
        Field::ReadOnly => {
            "Built-in: block edits, shell and plugins. External ACP processes are trusted, not sandboxed"
        }
        Field::Commands => {
            "ask (default), deny, or risk_assessed (auto-allow low-risk commands); high-risk commands stay blocked"
        }
        Field::Plugins => {
            "Discover plugins and request execution; every AI plugin command needs approval"
        }
        Field::AllowedPlugins => {
            "Comma-separated plugin names; empty = all enabled, except explicit denials in the Plugins menu"
        }
        Field::Skills => {
            "Comma-separated enabled skill IDs to attach to every prompt; others can be read on demand"
        }
        Field::SubAgents => {
            "Allow supervised delegation; nested coordinators must enable this too; root is depth 0"
        }
        Field::MaxDelegations => {
            "1–32 total specialist tasks shared across the entire root turn, including grandchildren"
        }
        Field::Timeout => {
            "Root turn and inherited specialist deadline, 10–1800 seconds (including approval waits)"
        }
        Field::AutoContext => {
            "Attach the open file and selection to prompts and descendant task briefs"
        }
        Field::Provider => {
            "auto (legacy Kilo/custom), router (pinned authenticated NoKey Router), kilo, or custom; does not change external ACP providers"
        }
        Field::MaxDepth => {
            "1–3; root = 0, child = 1, grandchild = 2. Restrictions and deadline propagate at every level"
        }
        Field::Concurrency => {
            "1–8 live tasks including waiting coordinators. A full tree fails new delegation immediately instead of deadlocking"
        }
        Field::StallTimeout => {
            "15–600 seconds without agent activity; waits on children/approvals/plugins are exempt, but absolute deadlines still apply"
        }
        Field::GeneratedTools => {
            "Validated, bounded JSON/text transformation functions only. No generated code, shell, filesystem/network I/O or eval"
        }
        Field::OutputTokens => {
            "256–16384 requested completion tokens; router/OpenAI max_tokens; inherited workers cannot increase it"
        }
    }
}

impl App {
    pub fn reload_skills(&mut self) {
        self.skills =
            crate::skills::discover(&self.explorer.root, &self.preferences.disabled_skills);
    }

    pub(crate) fn apply_preferences(&mut self, prefs: Preferences) -> Result<()> {
        prefs.validate()?;
        let mut check = crate::agent::Agent::new(self.explorer.root.clone());
        check
            .configure(prefs.agent.clone(), String::new())
            .map_err(anyhow::Error::msg)?;
        let restart = self.agent.options != prefs.agent;
        if (restart || self.preferences.sub_agents != prefs.sub_agents)
            && (self.agent.is_working() || self.automation.busy())
        {
            bail!("Stop the AI turn before changing agent or sub-agent settings");
        }
        if restart {
            self.agent.stop();
            self.automation.bridge = None;
        }
        self.settings.minimap = prefs.editor.minimap;
        self.settings.whitespace = prefs.editor.whitespace;
        self.settings.indent_guides = prefs.editor.indent_guides;
        self.settings.line_numbers = prefs.editor.line_numbers;
        self.settings.word_wrap = prefs.editor.word_wrap;
        self.settings.toolbar = prefs.editor.toolbar;
        if !prefs.editor.theme.is_empty() {
            self.set_theme(&prefs.editor.theme);
        }
        self.preferences = prefs;
        self.reload_skills();
        if restart || !self.agent.running() {
            self.agent
                .configure(
                    self.preferences.agent.clone(),
                    crate::skills::context(&self.skills, &self.preferences.agent.skills),
                )
                .map_err(anyhow::Error::msg)?;
        }
        self.management.load_error = None;
        Ok(())
    }

    pub(crate) fn ensure_preferences_writable(&self) -> Result<()> {
        let path = crate::search::absolute_path(&config::settings_path(&self.explorer.root));
        if self
            .editors
            .iter()
            .any(|e| e.doc.path.as_ref() == Some(&path) && e.doc.is_dirty())
        {
            bail!("Workspace settings JSON has unsaved edits; save it and reload settings first");
        }
        if self.management.load_error.is_some() {
            bail!("Fix the invalid settings JSON and reload it before saving menu changes");
        }
        Ok(())
    }

    /// Validate and write before mutating live preferences. No apparent
    /// success when the configuration directory is read-only.
    pub fn save_preferences(&mut self, prefs: Preferences) -> Result<()> {
        self.ensure_preferences_writable()?;
        prefs.validate()?;
        let mut check = crate::agent::Agent::new(self.explorer.root.clone());
        check
            .configure(prefs.agent.clone(), String::new())
            .map_err(anyhow::Error::msg)?;
        if (self.agent.options != prefs.agent || self.preferences.sub_agents != prefs.sub_agents)
            && (self.agent.is_working() || self.automation.busy())
        {
            bail!("Stop the AI turn before changing agent or sub-agent settings");
        }
        prefs.save(&self.explorer.root)?;
        self.apply_preferences(prefs)?;
        Ok(())
    }

    pub fn reload_preferences(&mut self) {
        match Preferences::load(&self.explorer.root).and_then(|p| self.apply_preferences(p)) {
            Ok(()) => self.toast("Settings reloaded", ToastKind::Success),
            Err(e) => {
                self.management.load_error = Some(e.to_string());
                self.toast(format!("Settings: {e:#}"), ToastKind::Error);
            }
        }
    }

    pub(crate) fn persist_editor_preferences(&mut self) {
        let mut prefs = self.preferences.clone();
        prefs.editor = config::EditorConfig {
            theme: self.theme.name.clone(),
            minimap: self.settings.minimap,
            whitespace: self.settings.whitespace,
            indent_guides: self.settings.indent_guides,
            line_numbers: self.settings.line_numbers,
            word_wrap: self.settings.word_wrap,
            toolbar: self.settings.toolbar,
        };
        if let Err(e) = self.save_preferences(prefs) {
            self.toast(format!("Settings not saved: {e:#}"), ToastKind::Error);
        }
    }

    pub(crate) fn management_items(&mut self, mode: PaletteMode) -> (Vec<String>, Vec<String>) {
        let mut rows: Vec<(String, String)> = Vec::new();
        let mut row = |title: String, hint: String| rows.push((title, hint));
        match mode {
            PaletteMode::Settings => {
                for (label, description) in [
                    (
                        "AI agent configuration",
                        "Backend, model, API endpoint, instructions and permissions",
                    ),
                    ("Sub-agents", "Create and configure specialist profiles"),
                    ("Skills", "Manage local Markdown instruction skills"),
                    ("Plugins", "Commands, enable/disable and AI access"),
                    ("Color theme", "Choose an editor theme"),
                ] {
                    row(label.into(), description.into());
                }
                for (label, v) in [
                    ("Minimap", on(self.settings.minimap)),
                    ("Render whitespace", on(self.settings.whitespace)),
                    ("Indent guides", on(self.settings.indent_guides)),
                    ("Line numbers", on(self.settings.line_numbers)),
                    ("Word wrap", auto(self.settings.word_wrap)),
                    ("Touch toolbar", auto(self.settings.toolbar)),
                ] {
                    row(
                        format!("{label}: {v}"),
                        "Enter to change; saved for this workspace".into(),
                    );
                }
                row(
                    "Open Workspace Settings JSON".into(),
                    config::settings_path(&self.explorer.root)
                        .display()
                        .to_string(),
                );
                row(
                    "Reload Settings from Disk".into(),
                    "Applies validated user defaults and workspace overrides".into(),
                );
                row(
                    "Open User Defaults JSON".into(),
                    config::config_dir()
                        .join("settings.json")
                        .display()
                        .to_string(),
                );
            }
            PaletteMode::AgentSettings | PaletteMode::SubAgentSettings => {
                let sub = mode == PaletteMode::SubAgentSettings;
                let profile = self
                    .management
                    .sub_agent
                    .as_ref()
                    .and_then(|id| self.preferences.sub_agents.iter().find(|p| &p.id == id));
                let config = if sub {
                    profile.map(|p| &p.agent).unwrap_or(&self.preferences.agent)
                } else {
                    &self.preferences.agent
                };
                row(
                    if sub {
                        "← Sub-agents"
                    } else {
                        "← Settings"
                    }
                    .into(),
                    String::new(),
                );
                if sub {
                    row(
                        format!("Enabled: {}", on(profile.is_some_and(|p| p.enabled))),
                        "Enter to toggle".into(),
                    );
                    row(
                        format!("Name: {}", profile.map(|p| p.name.as_str()).unwrap_or("")),
                        "Display name".into(),
                    );
                    row(
                        format!(
                            "Description: {}",
                            profile.map(|p| p.description.as_str()).unwrap_or("")
                        ),
                        "Helps the main AI choose a specialist".into(),
                    );
                }
                for &(field, label) in FIELDS.iter().skip(if sub { 2 } else { 0 }) {
                    let v = value(field, config);
                    row(
                        format!(
                            "{label}: {}",
                            if v.is_empty() {
                                "(default / empty)"
                            } else {
                                &v
                            }
                        ),
                        hint(field).into(),
                    );
                }
                if sub {
                    row("Delete Sub-agent…".into(), "Requires confirmation".into());
                } else {
                    row("Use NoKey Router".into(), "Built-in · free:code · pinned HTTPS /v1 endpoint; preserves other permission settings".into());
                    let status = if std::env::var(crate::credentials::ROUTER_ENV)
                        .is_ok_and(|s| !s.trim().is_empty())
                    {
                        "ROUTER_API_KEY environment override present"
                    } else if crate::credentials::path().exists() {
                        "Saved locally; never shown or copied into workspace settings"
                    } else {
                        "Not saved; enter locally in a masked field to save and select the router"
                    };
                    row("Save Router API Key…".into(), status.into());
                    row(
                        "Remove Saved Router API Key".into(),
                        "Does not unset an environment override".into(),
                    );
                    row("Show Agent Overwatch".into(),"Task tree, progress, deadlines, terminal results and blockers in the agent panel".into());
                }
            }
            PaletteMode::SubAgents => {
                row("← Settings".into(), String::new());
                row(
                    "New Sub-agent…".into(),
                    "Add an isolated built-in specialist".into(),
                );
                for p in &self.preferences.sub_agents {
                    row(
                        format!("[{}] {} ({})", on(p.enabled), p.name, p.id),
                        p.description.clone(),
                    );
                }
            }
            PaletteMode::Skills => {
                self.reload_skills();
                row("← Settings".into(), String::new());
                row(
                    "New Skill…".into(),
                    "Create .termed/skills/<id>/SKILL.md and edit it".into(),
                );
                row(
                    "Reload Skills".into(),
                    "Discover workspace and user skills".into(),
                );
                for skill in &self.skills {
                    row(
                        format!("[{}] {} ({})", on(skill.enabled), skill.name, skill.id),
                        skill.description.clone(),
                    );
                }
            }
            PaletteMode::SkillActions => {
                row("← Skills".into(), String::new());
                if let Some(skill) = self
                    .management
                    .skill
                    .as_ref()
                    .and_then(|id| self.skills.iter().find(|s| &s.id == id))
                {
                    row(
                        format!("{} skill", if skill.enabled { "Disable" } else { "Enable" }),
                        "Disabled skills are never exposed to the AI".into(),
                    );
                    row(
                        "Edit Skill Instructions".into(),
                        skill.path.display().to_string(),
                    );
                    row(
                        format!(
                            "{} to primary agent",
                            if self.preferences.agent.skills.contains(&skill.id) {
                                "Detach"
                            } else {
                                "Attach"
                            }
                        ),
                        "Include this skill on every new prompt".into(),
                    );
                    row(
                        "Delete Skill…".into(),
                        if skill.workspace {
                            "Requires confirmation; removes SKILL.md only"
                        } else {
                            "Global skills can be disabled, not deleted here"
                        }
                        .into(),
                    );
                }
            }
            PaletteMode::PluginActions => {
                row("← Plugins".into(), String::new());
                if let Some(p) = self
                    .management
                    .plugin
                    .and_then(|pi| self.plugins.plugins.get(pi))
                {
                    row(
                        format!("{} plugin", if p.enabled { "Disable" } else { "Enable" }),
                        p.status(),
                    );
                    row("Run Plugin Command…".into(), p.description.clone());
                    row(
                        format!(
                            "AI access: {}",
                            on(!self.preferences.agent.denied_plugins.contains(&p.name))
                        ),
                        "AI execution always needs approval; applies to workers too".into(),
                    );
                    row("Open Plugin Manifest".into(), p.dir.display().to_string());
                }
            }
            PaletteMode::PluginCommands => {
                row("← Plugin".into(), String::new());
                if let Some(p) = self
                    .management
                    .plugin
                    .and_then(|pi| self.plugins.plugins.get(pi))
                {
                    for c in &p.commands {
                        row(c.title.clone(), c.keys.clone().unwrap_or_default());
                    }
                }
            }
            PaletteMode::ConfirmRemoveSkill | PaletteMode::ConfirmRemoveSubAgent => {
                row("Cancel".into(), "Keep it".into());
                row("Delete".into(), "This cannot be undone".into());
            }
            PaletteMode::Input => {}
            _ => {}
        }
        rows.into_iter().unzip()
    }

    fn edit_value(&mut self, target: EditTarget, title: &str, hint: &str, value: String) {
        self.management.edit = Some(Edit {
            title: title.into(),
            hint: hint.into(),
            value,
            secret: matches!(&target, EditTarget::RouterKey),
            target,
        });
        self.open_palette(PaletteMode::Input);
    }

    fn select_field(&mut self, field: Field, label: &str, sub: bool) -> Result<()> {
        let mut prefs = self.preferences.clone();
        let id = self.management.sub_agent.clone().unwrap_or_default();
        let config = if sub {
            &mut prefs
                .sub_agents
                .iter_mut()
                .find(|p| p.id == id)
                .ok_or_else(|| anyhow::anyhow!("Profile no longer exists"))?
                .agent
        } else {
            &mut prefs.agent
        };
        let toggle = match field {
            Field::ReadOnly => {
                config.read_only = !config.read_only;
                true
            }
            Field::Plugins => {
                config.plugins = !config.plugins;
                true
            }
            Field::SubAgents => {
                config.sub_agents = !config.sub_agents;
                true
            }
            Field::GeneratedTools => {
                config.generated_tools = !config.generated_tools;
                true
            }
            Field::AutoContext => {
                config.auto_context = !config.auto_context;
                true
            }
            Field::Commands => {
                config.commands = match config.commands {
                    CommandPolicy::Ask => CommandPolicy::Deny,
                    CommandPolicy::Deny => CommandPolicy::RiskAssessed,
                    CommandPolicy::RiskAssessed => CommandPolicy::Ask,
                };
                true
            }
            _ => false,
        };
        if toggle {
            self.save_preferences(prefs)?;
            self.open_palette(if sub {
                PaletteMode::SubAgentSettings
            } else {
                PaletteMode::AgentSettings
            });
        } else {
            let value = value(field, config);
            self.edit_value(
                if sub {
                    EditTarget::Sub(id, field)
                } else {
                    EditTarget::Main(field)
                },
                label,
                hint(field),
                value,
            );
        }
        Ok(())
    }

    pub(crate) fn cancel_setting_edit(&mut self) {
        let mode = self.management.edit.as_ref().map(|e| match e.target {
            EditTarget::CreateSkill => PaletteMode::Skills,
            EditTarget::CreateSub => PaletteMode::SubAgents,
            EditTarget::Main(_) | EditTarget::RouterKey => PaletteMode::AgentSettings,
            _ => PaletteMode::SubAgentSettings,
        });
        self.management.edit = None;
        if let Some(mode) = mode {
            self.open_palette(mode);
        }
    }

    pub(crate) fn management_accept(
        &mut self,
        mode: PaletteMode,
        index: Option<usize>,
        input: String,
    ) {
        if mode == PaletteMode::Input {
            if let Some(edit) = &mut self.management.edit {
                edit.value = input.clone();
            }
        }
        let result = self.management_action(mode, index, input);
        if let Err(e) = result {
            self.toast(format!("{e:#}"), ToastKind::Error);
            self.open_palette(mode);
        }
        if let (Some(index), Some(p)) = (index, self.palette.as_mut()) {
            if p.mode == mode && mode != PaletteMode::Input {
                if let Some(selected) = p.filtered.iter().position(|item| item.index == index) {
                    p.selected = selected;
                }
            }
        }
    }

    fn management_action(
        &mut self,
        mode: PaletteMode,
        index: Option<usize>,
        input: String,
    ) -> Result<()> {
        let i = index.unwrap_or(0);
        match mode {
            PaletteMode::Settings => match i {
                0 => self.open_palette(PaletteMode::AgentSettings), 1 => self.open_palette(PaletteMode::SubAgents),
                2 => self.open_palette(PaletteMode::Skills), 3 => self.open_palette(PaletteMode::Plugins), 4 => self.open_palette(PaletteMode::Themes),
                5..=10 => {
                    let mut prefs = self.preferences.clone();
                    match i { 5=>prefs.editor.minimap=!prefs.editor.minimap, 6=>prefs.editor.whitespace=!prefs.editor.whitespace,
                        7=>prefs.editor.indent_guides=!prefs.editor.indent_guides, 8=>prefs.editor.line_numbers=!prefs.editor.line_numbers,
                        9=>prefs.editor.word_wrap=cycle(prefs.editor.word_wrap), _=>prefs.editor.toolbar=cycle(prefs.editor.toolbar) }
                    self.save_preferences(prefs)?; self.open_palette(mode);
                }
                11 => { let path = config::settings_path(&self.explorer.root); if !path.exists() { self.preferences.save(&self.explorer.root)?; } self.open_file(&path); }
                12 => { self.reload_preferences(); self.open_palette(mode); }
                13 => {
                    let path = config::config_dir().join("settings.json");
                    if !path.exists() { std::fs::create_dir_all(path.parent().unwrap())?; std::fs::write(&path,"{}\n")?; }
                    self.open_file(&path);
                }
                _ => {}
            },
            PaletteMode::AgentSettings | PaletteMode::SubAgentSettings => {
                let sub = mode == PaletteMode::SubAgentSettings;
                if i == 0 { self.open_palette(if sub { PaletteMode::SubAgents } else { PaletteMode::Settings }); }
                else if sub && i <= 3 {
                    let id = self.management.sub_agent.clone().unwrap_or_default();
                    let mut prefs = self.preferences.clone();
                    let profile = prefs.sub_agents.iter_mut().find(|p| p.id == id).ok_or_else(|| anyhow::anyhow!("Profile not found"))?;
                    match i {
                        1 => { profile.enabled = !profile.enabled; self.save_preferences(prefs)?; self.open_palette(mode); }
                        2 => self.edit_value(EditTarget::SubName(id),"Sub-agent name","Display name",profile.name.clone()),
                        _ => self.edit_value(EditTarget::SubDescription(id),"Sub-agent description","When should the main agent delegate to this specialist?",profile.description.clone()),
                    }
                } else {
                    let field_index = if sub { i - 2 } else { i - 1 };
                    if let Some(&(field,label)) = FIELDS.get(field_index) { self.select_field(field,label,sub)?; }
                    else if sub { self.open_palette(PaletteMode::ConfirmRemoveSubAgent); }
                    else { match field_index - FIELDS.len() {
                        0 => {let mut prefs=self.preferences.clone();prefs.agent.use_router();self.save_preferences(prefs)?;self.open_palette(mode);}
                        1 => {
                            if self.agent.is_working()||self.automation.busy(){bail!("Stop the AI turn before replacing credentials");}
                            self.edit_value(EditTarget::RouterKey,"Save Router API Key (hidden)","Saves locally and selects the built-in router for this workspace. ROUTER_API_KEY wins. Esc cancels.",String::new());
                        }
                        2 => {
                            if self.agent.is_working()||self.automation.busy(){bail!("Stop the AI turn before removing credentials");}
                            crate::credentials::Store::default().remove()?;
                            self.toast("Saved router key removed; an exported ROUTER_API_KEY still overrides",ToastKind::Success);
                            self.open_palette(mode);
                        }
                        3 => {
                            let summary=serde_json::to_string_pretty(&self.automation.overwatch.snapshot(None))?;
                            self.agent.entries.push(crate::agent::Entry::Notice{text:format!("Agent overwatch\n{summary}"),error:false});
                            self.palette=None;self.show_agent=true;self.focus=crate::app::Focus::Agent;
                        }
                        _=>{}
                    }}
                }
            }
            PaletteMode::SubAgents => match i {
                0 => self.open_palette(PaletteMode::Settings),
                1 => self.edit_value(EditTarget::CreateSub,"New sub-agent","Unique ID: letters, digits, '-' or '_'; built-in worker with its own model and role",String::new()),
                _ => if let Some(profile) = self.preferences.sub_agents.get(i-2) { self.management.sub_agent=Some(profile.id.clone()); self.open_palette(PaletteMode::SubAgentSettings); },
            },
            PaletteMode::Skills => match i {
                0 => self.open_palette(PaletteMode::Settings),
                1 => self.edit_value(EditTarget::CreateSkill,"New skill","Unique ID: letters, digits, '-' or '_'; opens SKILL.md for editing",String::new()),
                2 => { self.reload_skills(); self.open_palette(mode); }
                _ => if let Some(skill) = self.skills.get(i-3) { self.management.skill=Some(skill.id.clone()); self.open_palette(PaletteMode::SkillActions); },
            },
            PaletteMode::SkillActions => {
                if i == 0 { self.open_palette(PaletteMode::Skills); return Ok(()); }
                let skill = self.management.skill.as_ref().and_then(|id| self.skills.iter().find(|s| &s.id == id)).cloned().ok_or_else(|| anyhow::anyhow!("Skill no longer exists"))?;
                match i {
                    1 => {
                        let mut prefs = self.preferences.clone();
                        prefs.disabled_skills.retain(|id| id != &skill.id);
                        if skill.enabled { prefs.disabled_skills.push(skill.id); }
                        self.save_preferences(prefs)?; self.open_palette(mode);
                    }
                    2 => self.open_file(&skill.path),
                    3 => {
                        let mut prefs = self.preferences.clone();
                        if prefs.agent.skills.contains(&skill.id) { prefs.agent.skills.retain(|id| id != &skill.id); }
                        else { prefs.agent.skills.push(skill.id); }
                        self.save_preferences(prefs)?; self.open_palette(mode);
                    }
                    4 => { if !skill.workspace { bail!("Global skills can only be disabled here"); } self.open_palette(PaletteMode::ConfirmRemoveSkill); }
                    _ => {}
                }
            }
            PaletteMode::PluginActions => {
                if i == 0 { self.open_palette(PaletteMode::Plugins); return Ok(()); }
                let pi = self.management.plugin.ok_or_else(|| anyhow::anyhow!("Plugin not selected"))?;
                let plugin = self.plugins.plugins.get(pi).cloned().ok_or_else(|| anyhow::anyhow!("Plugin no longer exists"))?;
                match i {
                    1 => { self.cancel_automation(); self.plugins.try_set_enabled(pi,!plugin.enabled)?; self.open_palette(mode); }
                    2 => { if !plugin.enabled || !plugin.runnable() { bail!("{}",plugin.status()); } self.open_palette(PaletteMode::PluginCommands); }
                    3 => {
                        let mut prefs=self.preferences.clone();
                        if prefs.agent.denied_plugins.contains(&plugin.name) { prefs.agent.denied_plugins.retain(|n| n != &plugin.name); }
                        else { prefs.agent.denied_plugins.push(plugin.name); }
                        self.save_preferences(prefs)?; self.open_palette(mode);
                    }
                    4 => { let native=plugin.dir.join("plugin.json"); self.open_file(&if native.exists() { native } else { plugin.dir.join("package.json") }); }
                    _ => {}
                }
            }
            PaletteMode::PluginCommands => {
                if i==0 { self.open_palette(PaletteMode::PluginActions); }
                else if let Some(pi)=self.management.plugin { self.run_plugin(pi,i-1); }
            }
            PaletteMode::ConfirmRemoveSkill => {
                if i==1 {
                    let skill=self.management.skill.as_ref().and_then(|id| self.skills.iter().find(|s| &s.id==id)).cloned().ok_or_else(||anyhow::anyhow!("Skill not found"))?;
                    if self.editors.iter().any(|e| e.doc.path.as_ref()==Some(&skill.path) && e.doc.is_dirty()) { bail!("Save or discard the skill's unsaved edits before deleting it"); }
                    if self.agent.is_working() || self.automation.busy() { bail!("Stop the AI turn before deleting skills"); }
                    let previous = self.preferences.clone();
                    let mut prefs=self.preferences.clone();
                    prefs.agent.skills.retain(|id| id != &skill.id);
                    for profile in &mut prefs.sub_agents { profile.agent.skills.retain(|id| id != &skill.id); }
                    prefs.disabled_skills.retain(|id| id != &skill.id);
                    self.save_preferences(prefs)?;
                    if let Err(e) = crate::skills::remove(&self.explorer.root, &skill) {
                        let _ = self.save_preferences(previous);
                        return Err(e);
                    }
                    self.editors.retain(|e| e.doc.path.as_ref() != Some(&skill.path));
                    self.active = self.active.min(self.editors.len().saturating_sub(1));
                    self.reload_skills(); self.open_palette(PaletteMode::Skills);
                } else { self.open_palette(PaletteMode::SkillActions); }
            }
            PaletteMode::ConfirmRemoveSubAgent => {
                if i==1 {
                    let mut prefs=self.preferences.clone(); prefs.sub_agents.retain(|p|Some(&p.id)!=self.management.sub_agent.as_ref());
                    self.save_preferences(prefs)?; self.open_palette(PaletteMode::SubAgents);
                } else { self.open_palette(PaletteMode::SubAgentSettings); }
            }
            PaletteMode::Input => self.accept_edit(input)?,
            _ => {}
        }
        Ok(())
    }

    fn accept_edit(&mut self, input: String) -> Result<()> {
        let edit = self
            .management
            .edit
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No setting selected"))?;
        let input = input.trim();
        let mut prefs = self.preferences.clone();
        let return_mode = match edit.target {
            EditTarget::RouterKey => {
                if self.agent.is_working() || self.automation.busy() {
                    bail!("Stop the AI turn before replacing credentials");
                }
                crate::credentials::Store::default().save(input)?;
                // Saving a router credential is an explicit request to use it.
                // Do not leave the editor silently connected to a different
                // backend; preserve a router model the user already selected.
                let already_router = prefs.agent.uses_router();
                if !already_router || prefs.agent.backend != "builtin" {
                    let model = prefs.agent.model.clone();
                    prefs.agent.use_router();
                    if already_router && !model.is_empty() {
                        prefs.agent.model = model;
                    }
                }
                // The current selection can be a launch-only --router override,
                // so persist even if the router was already active in memory.
                let selection = self.save_preferences(prefs);
                // Drop the masked input even if preferences cannot be written:
                // the credential was already saved, so reporting otherwise (or
                // keeping it in the palette) would be misleading.
                self.management.edit = None;
                self.open_palette(PaletteMode::AgentSettings);
                match selection {
                    Err(e) => self.toast(
                        format!("Router key saved locally, but router settings could not be saved: {e:#}. Fix the settings, then choose Use NoKey Router."),
                        ToastKind::Error,
                    ),
                    Ok(()) if std::env::var(crate::credentials::ROUTER_ENV)
                        .is_ok_and(|key| !key.trim().is_empty()) => self.toast(
                        "Router selected; the ROUTER_API_KEY environment override is still in use instead of the saved key",
                        ToastKind::Info,
                    ),
                    Ok(()) => self.toast(
                        "Router key saved privately and NoKey Router selected",
                        ToastKind::Success,
                    ),
                }
                return Ok(());
            }
            EditTarget::CreateSkill => {
                if self.skills.iter().any(|s| s.id == input) {
                    bail!("A skill with that ID already exists");
                }
                let path = crate::skills::create(&self.explorer.root, input)?;
                self.reload_skills();
                self.open_file(&path);
                self.management.edit = None;
                return Ok(());
            }
            EditTarget::CreateSub => {
                if !config::valid_id(input) || prefs.sub_agents.iter().any(|p| p.id == input) {
                    bail!("Use a unique ID containing letters, digits, '-' or '_'");
                }
                prefs.sub_agents.push(SubAgentConfig {
                    id: input.into(),
                    name: input.replace('-', " "),
                    ..Default::default()
                });
                self.management.sub_agent = Some(input.into());
                PaletteMode::SubAgentSettings
            }
            EditTarget::SubName(id) => {
                let profile = prefs
                    .sub_agents
                    .iter_mut()
                    .find(|p| p.id == id)
                    .ok_or_else(|| anyhow::anyhow!("Profile not found"))?;
                profile.name = input.into();
                PaletteMode::SubAgentSettings
            }
            EditTarget::SubDescription(id) => {
                let profile = prefs
                    .sub_agents
                    .iter_mut()
                    .find(|p| p.id == id)
                    .ok_or_else(|| anyhow::anyhow!("Profile not found"))?;
                profile.description = input.into();
                PaletteMode::SubAgentSettings
            }
            EditTarget::Main(field) => {
                set_field(&mut prefs.agent, field, input)?;
                PaletteMode::AgentSettings
            }
            EditTarget::Sub(id, field) => {
                let profile = prefs
                    .sub_agents
                    .iter_mut()
                    .find(|p| p.id == id)
                    .ok_or_else(|| anyhow::anyhow!("Profile not found"))?;
                set_field(&mut profile.agent, field, input)?;
                PaletteMode::SubAgentSettings
            }
        };
        self.save_preferences(prefs)?;
        self.management.edit = None;
        self.open_palette(return_mode);
        Ok(())
    }
}

fn set_field(a: &mut AgentConfig, field: Field, input: &str) -> Result<()> {
    match field {
        Field::Backend => a.backend = input.into(),
        Field::Provider => a.provider = input.into(),
        Field::MaxDepth => {
            a.max_agent_depth = input
                .parse()
                .map_err(|_| anyhow::anyhow!("Enter a depth from 1 to 3"))?
        }
        Field::Concurrency => {
            a.max_concurrent_tasks = input
                .parse()
                .map_err(|_| anyhow::anyhow!("Enter a concurrency limit from 1 to 8"))?
        }
        Field::StallTimeout => {
            a.stall_timeout_secs = input
                .parse()
                .map_err(|_| anyhow::anyhow!("Enter a stall timeout from 15 to 600 seconds"))?
        }
        Field::OutputTokens => {
            a.max_output_tokens = input
                .parse()
                .map_err(|_| anyhow::anyhow!("Enter an output token budget from 256 to 16384"))?
        }
        Field::Command => a.command = input.into(),
        Field::Model => a.model = input.into(),
        Field::Endpoint => a.endpoint = input.trim_end_matches('/').into(),
        Field::ApiKeyEnv => a.api_key_env = input.into(),
        Field::Instructions => a.instructions = input.into(),
        Field::MaxSteps => {
            a.max_steps = input
                .parse()
                .map_err(|_| anyhow::anyhow!("Enter a step budget from 1 to 100"))?
        }
        Field::AllowedPlugins => a.allowed_plugins = csv(input),
        Field::Skills => a.skills = csv(input),
        Field::MaxDelegations => {
            a.max_delegations = input
                .parse()
                .map_err(|_| anyhow::anyhow!("Enter a delegation budget from 1 to 32"))?
        }
        Field::Timeout => {
            a.task_timeout_secs = input
                .parse()
                .map_err(|_| anyhow::anyhow!("Enter a timeout in seconds"))?
        }
        _ => bail!("This setting is a toggle"),
    }
    Ok(())
}
