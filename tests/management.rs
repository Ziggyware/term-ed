mod common;

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use termed::app::{App, PaletteMode};
use termed::commands::Command;
use termed::config::{AgentConfig, CommandPolicy, Preferences, settings_path, user_settings_path};

fn project(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "termed-management-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}
fn app(root: &std::path::Path) -> App {
    App::new(root.to_path_buf(), vec![], "Visual Studio Dark", false)
}
fn key(app: &mut App, code: KeyCode) {
    app.handle_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}
fn choose(app: &mut App, prefix: &str) {
    let p = app.palette.as_mut().expect("menu open");
    p.selected = p
        .filtered
        .iter()
        .position(|s| p.items[s.index].starts_with(prefix))
        .unwrap_or_else(|| panic!("{prefix}: {:?}", p.items));
    key(app, KeyCode::Enter);
}
fn type_value(app: &mut App, value: &str) {
    app.handle_event(Event::Paste(value.into()));
    key(app, KeyCode::Enter);
}

#[test]
fn settings_menu_saves_and_reloads_validated_values() {
    if common::isolate("settings_menu_saves_and_reloads_validated_values") {
        return;
    }
    let root = project("persist");
    let mut a = app(&root);
    a.run(Command::Settings);
    choose(&mut a, "Minimap:");
    assert!(!a.settings.minimap);
    choose(&mut a, "AI agent configuration");
    choose(&mut a, "Backend:");
    assert_eq!(a.palette.as_ref().unwrap().mode, PaletteMode::Input);
    type_value(&mut a, "builtin");
    choose(&mut a, "Model:");
    type_value(&mut a, "local/test-model");
    choose(&mut a, "API base URL:");
    type_value(&mut a, "http://127.0.0.1:9999/v1");
    choose(&mut a, "API key environment variable:");
    type_value(&mut a, "MY_AI_KEY");
    choose(&mut a, "Step budget:");
    type_value(&mut a, "9");
    let b = app(&root);
    assert!(!b.settings.minimap);
    assert_eq!(b.agent.options.backend, "builtin");
    assert_eq!(b.agent.options.model, "local/test-model");
    assert_eq!(b.agent.options.max_steps, 9);
    assert_eq!(b.agent.options.api_key_env, "MY_AI_KEY");
    assert!(user_settings_path().is_file());
    assert!(!settings_path(&root).exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(user_settings_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(user_settings_path().parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    // A different folder's old settings cannot override the user settings.
    let other = project("persist-other-folder");
    std::fs::create_dir_all(other.join(".termed")).unwrap();
    std::fs::write(settings_path(&other), "{ stale and invalid").unwrap();
    let c = app(&other);
    assert!(c.management.load_error.is_none());
    assert!(!c.settings.minimap);
    assert_eq!(c.agent.options.backend, "builtin");
    assert_eq!(c.agent.options.model, "local/test-model");
    assert_eq!(c.agent.options.max_steps, 9);
    assert_eq!(c.agent.options.api_key_env, "MY_AI_KEY");
    assert!(c.toasts.iter().any(|toast| {
        toast
            .text
            .contains("legacy workspace restrictions and settings are not applied")
    }));
    assert_eq!(
        std::fs::read_to_string(settings_path(&other)).unwrap(),
        "{ stale and invalid"
    );

    // Invalid edits don't corrupt the previous saved value.
    choose(&mut a, "Step budget:");
    type_value(&mut a, "0");
    assert_eq!(a.palette.as_ref().unwrap().mode, PaletteMode::Input);
    assert_eq!(Preferences::load(&root).unwrap().agent.max_steps, 9);
}

#[test]
fn invalid_json_is_preserved_and_reported() {
    if common::isolate("invalid_json_is_preserved_and_reported") {
        return;
    }
    let root = project("invalid");
    std::fs::create_dir_all(user_settings_path().parent().unwrap()).unwrap();
    std::fs::write(user_settings_path(), "{ invalid").unwrap();
    let mut a = app(&root);
    assert!(a.management.load_error.is_some());
    assert!(a.save_preferences(Preferences::default()).is_err());
    assert_eq!(
        std::fs::read_to_string(user_settings_path()).unwrap(),
        "{ invalid"
    );
    std::fs::write(
        user_settings_path(),
        r#"{"agent":{"backend":"builtin","read_only":true}}"#,
    )
    .unwrap();
    a.reload_preferences();
    assert!(a.management.load_error.is_none());
    assert!(a.agent.options.read_only);
}

#[test]
fn sub_agent_profiles_can_be_created_edited_disabled_and_removed() {
    if common::isolate("sub_agent_profiles_can_be_created_edited_disabled_and_removed") {
        return;
    }
    let root = project("sub-agents");
    let mut a = app(&root);
    a.run(Command::SubAgents);
    choose(&mut a, "New Sub-agent");
    type_value(&mut a, "security");
    choose(&mut a, "Name:");
    type_value(&mut a, "Security reviewer");
    choose(&mut a, "Instructions:");
    type_value(
        &mut a,
        "Look for unsafe input handling and cite exact lines.",
    );
    choose(&mut a, "Model:");
    type_value(&mut a, "security/model");
    choose(&mut a, "Enabled:");
    let p = Preferences::load(&root)
        .unwrap()
        .sub_agents
        .into_iter()
        .find(|p| p.id == "security")
        .unwrap();
    assert_eq!(p.name, "Security reviewer");
    assert!(!p.enabled);
    assert_eq!(p.agent.model, "security/model");
    assert!(p.agent.instructions.contains("unsafe input"));
    choose(&mut a, "Delete Sub-agent");
    choose(&mut a, "Cancel");
    assert!(a.preferences.sub_agents.iter().any(|p| p.id == "security"));
    choose(&mut a, "Delete Sub-agent");
    choose(&mut a, "Delete");
    assert!(
        !Preferences::load(&root)
            .unwrap()
            .sub_agents
            .iter()
            .any(|p| p.id == "security")
    );
}

#[test]
fn skills_are_managed_and_injected_only_when_enabled() {
    if common::isolate("skills_are_managed_and_injected_only_when_enabled") {
        return;
    }
    let root = project("skills");
    let mut a = app(&root);
    a.run(Command::Skills);
    choose(&mut a, "New Skill");
    type_value(&mut a, "project-style");
    let path = root.join(".termed/skills/project-style/SKILL.md");
    assert!(path.is_file());
    std::fs::write(
        &path,
        "# Project style\n\nAlways test Unicode edge cases.\n",
    )
    .unwrap();
    // Reopen discovery to see edited instructions.
    a.run(Command::Skills);
    choose(&mut a, "[on] Project style");
    choose(&mut a, "Attach");
    assert!(a.agent.skill_context.contains("Unicode edge cases"));
    choose(&mut a, "Disable");
    assert!(!termed::skills::context(&a.skills, &a.preferences.agent.skills).contains("Unicode"));
    assert!(
        a.skills
            .iter()
            .any(|s| s.id == "project-style" && !s.enabled)
    );
    choose(&mut a, "Delete Skill");
    choose(&mut a, "Delete");
    assert!(!path.exists());
    assert!(!a.preferences.agent.skills.contains(&"project-style".into()));
}

#[test]
fn menus_render_at_desktop_and_phone_sizes_and_have_click_targets() {
    if common::isolate("menus_render_at_desktop_and_phone_sizes_and_have_click_targets") {
        return;
    }
    let root = project("layout");
    let mut a = app(&root);
    for (w, h) in [(140, 42), (80, 24), (46, 30), (28, 18)] {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| termed::ui::draw(f, &mut a, true)).unwrap();
        if w >= 80 {
            for cmd in [Command::Settings, Command::Plugins, Command::FindInFiles] {
                assert!(
                    a.layout
                        .click_items
                        .iter()
                        .any(|(r, c)| *c == cmd && r.width > 0)
                );
            }
        }
        for cmd in [
            Command::Settings,
            Command::Plugins,
            Command::AgentSettings,
            Command::SubAgents,
            Command::Skills,
        ] {
            a.run(cmd);
            t.draw(|f| termed::ui::draw(f, &mut a, true)).unwrap();
            assert!(a.layout.palette_list.width <= w);
            assert!(a.layout.palette_list.bottom() <= h);
            key(&mut a, KeyCode::Esc);
        }
    }
}

#[test]
fn sub_agents_cannot_widen_parent_permissions() {
    if common::isolate("sub_agents_cannot_widen_parent_permissions") {
        return;
    }
    let parent = AgentConfig {
        read_only: true,
        commands: CommandPolicy::Deny,
        plugins: false,
        allowed_plugins: vec!["stats".into()],
        ..Default::default()
    };
    let child = AgentConfig {
        read_only: false,
        commands: CommandPolicy::RiskAssessed,
        allowed_plugins: vec!["text-tools".into()],
        ..Default::default()
    }
    .restricted_by(&parent);
    assert!(child.read_only);
    assert!(!child.plugins);
    assert!(
        child.sub_agents,
        "Nesting is allowed only within inherited restrictions and depth/budgets"
    );
    assert_eq!(child.commands, CommandPolicy::Deny);
    let parent = AgentConfig {
        denied_plugins: vec!["stats".into()],
        ..Default::default()
    };
    assert!(
        AgentConfig::default()
            .restricted_by(&parent)
            .denied_plugins
            .contains(&"stats".into())
    );
}

#[test]
fn custom_agent_command_parsing_preserves_quoted_arguments() {
    if common::isolate("custom_agent_command_parsing_preserves_quoted_arguments") {
        return;
    }
    assert_eq!(
        termed::plugin::split_command(r#"python3 "/tmp/my agent.py" --label 'two words' """#),
        vec!["python3", "/tmp/my agent.py", "--label", "two words", ""]
    );
    assert!(termed::plugin::split_command("python3 'unterminated").is_empty());
    assert_eq!(
        termed::plugin::split_command(r#"C:\tools\agent.exe --acp"#),
        vec![r#"C:\tools\agent.exe"#, "--acp"]
    );
}

#[test]
fn worker_credentials_and_budgets_cannot_leak_or_expand() {
    if common::isolate("worker_credentials_and_budgets_cannot_leak_or_expand") {
        return;
    }
    let parent = AgentConfig {
        provider: "custom".into(),
        endpoint: "https://one.invalid/v1".into(),
        api_key_env: "ONE_KEY".into(),
        max_steps: 7,
        task_timeout_secs: 45,
        ..Default::default()
    };
    let child = AgentConfig {
        provider: "custom".into(),
        endpoint: "https://two.invalid/v1".into(),
        max_steps: 90,
        task_timeout_secs: 1000,
        ..Default::default()
    }
    .restricted_by(&parent);
    assert!(child.api_key_env.is_empty());
    assert_eq!(child.max_steps, 7);
    assert_eq!(child.task_timeout_secs, 45);
    assert_eq!(
        termed::config::SubAgentConfig::default()
            .agent
            .restricted_by(&parent)
            .api_key_env,
        "ONE_KEY"
    );
}

#[cfg(unix)]
#[test]
fn workspace_paths_reject_symlink_escapes_even_for_new_files() {
    if common::isolate("workspace_paths_reject_symlink_escapes_even_for_new_files") {
        return;
    }
    let root = project("symlink-root");
    let outside = project("symlink-outside");
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    assert!(termed::config::workspace_path(&root, "link/new.txt").is_err());
    assert!(termed::config::workspace_path(&root, "../outside.txt").is_err());
    assert!(termed::config::workspace_path(&root, "new/file.txt").is_ok());
}

#[test]
fn model_picker_does_not_overwrite_unsaved_settings_json() {
    if common::isolate("model_picker_does_not_overwrite_unsaved_settings_json") {
        return;
    }
    let root = project("model-dirty-settings");
    let mut prefs = Preferences::default();
    prefs.agent.backend = "builtin".into();
    prefs.agent.model = "old-model".into();
    prefs.save(&root).unwrap();
    let original = std::fs::read_to_string(user_settings_path()).unwrap();
    let mut a = app(&root);
    a.open_file(&user_settings_path());
    a.editor_mut().unwrap().insert_text(" ");
    a.agent.models = vec![("new-model".into(), "New model".into())];
    a.run(Command::AgentModel);
    choose(&mut a, "new-model");
    assert_eq!(a.preferences.agent.model, "old-model");
    assert_eq!(
        std::fs::read_to_string(user_settings_path()).unwrap(),
        original
    );
    assert!(a.editor().unwrap().doc.is_dirty());
}

#[cfg(unix)]
#[test]
fn settings_menu_preserves_dirty_json_opened_through_a_directory_symlink() {
    if common::isolate("settings_menu_preserves_dirty_json_opened_through_a_directory_symlink") {
        return;
    }
    let root = project("settings-symlink");
    let mut prefs = Preferences::default();
    prefs.save(&root).unwrap();
    let alias = root.join("configuration-alias");
    std::os::unix::fs::symlink(user_settings_path().parent().unwrap(), &alias).unwrap();
    let original = std::fs::read_to_string(user_settings_path()).unwrap();
    let mut a = app(&root);
    a.open_file(&alias.join("settings.json"));
    a.editor_mut().unwrap().insert_text(" ");
    prefs.editor.minimap = false;
    assert!(a.save_preferences(prefs).is_err());
    assert_eq!(
        std::fs::read_to_string(user_settings_path()).unwrap(),
        original
    );
    assert!(a.editor().unwrap().doc.is_dirty());
}

#[cfg(unix)]
#[test]
fn skill_aliases_cannot_bypass_unsaved_instruction_protection() {
    if common::isolate("skill_aliases_cannot_bypass_unsaved_instruction_protection") {
        return;
    }
    let root = project("skill-symlink");
    let path = termed::skills::create(&root, "original").unwrap();
    let alias = root.join(".termed/skills/alias");
    std::os::unix::fs::symlink(path.parent().unwrap(), &alias).unwrap();
    let mut a = app(&root);
    a.open_file(&alias.join("SKILL.md"));
    a.editor_mut().unwrap().insert_text("Unsaved instructions");
    a.management.skill = Some("alias".into());
    a.open_palette(PaletteMode::ConfirmRemoveSkill);
    choose(&mut a, "Delete");
    assert!(
        path.exists(),
        "do not delete instructions with unsaved edits"
    );
    assert!(a.editor().unwrap().doc.is_dirty());
    assert!(a.toasts.iter().any(|t| t.text.contains("unsaved edits")));
}

#[test]
fn router_setup_masks_input_and_provider_and_overwatch_controls_are_accessible() {
    if common::isolate(
        "router_setup_masks_input_and_provider_and_overwatch_controls_are_accessible",
    ) {
        return;
    }
    let root = project("router-menu");
    let mut a = app(&root);
    a.run(Command::AgentSettings);
    choose(&mut a, "Use NoKey Router");
    assert!(a.agent.options.uses_router());
    assert_eq!(a.agent.options.model, "free:code");
    assert_eq!(a.agent.options.backend, "builtin");
    choose(&mut a, "Save Router API Key");
    assert!(a.management.edit.as_ref().unwrap().secret);
    let secret = "test-secret-never-render-this";
    a.handle_event(Event::Paste(secret.into()));
    for (w, h) in [(140, 42), (46, 30)] {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| termed::ui::draw(f, &mut a, true))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(!rendered.contains(secret));
        assert!(rendered.contains('•'));
    }
    key(&mut a, KeyCode::Esc);
    assert_eq!(a.palette.as_ref().unwrap().mode, PaletteMode::AgentSettings);
    let settings = std::fs::read_to_string(user_settings_path()).unwrap();
    assert!(!settings.contains(secret));
    assert!(!settings.contains("router_api_key"));
    choose(&mut a, "Maximum agent depth:");
    type_value(&mut a, "3");
    assert_eq!(a.agent.options.max_agent_depth, 3);
    choose(&mut a, "Pure generated functions:");
    assert!(!a.agent.options.generated_tools);
    choose(&mut a, "Show Agent Overwatch");
    assert!(a.show_agent);
    assert!(a.agent.entries.iter().any(
        |e| matches!(e,termed::agent::Entry::Notice{text,..} if text.contains("Agent overwatch"))
    ));
}

#[test]
fn saving_router_key_selects_builtin_and_reports_override_or_selection_failure() {
    if common::isolate(
        "saving_router_key_selects_builtin_and_reports_override_or_selection_failure",
    ) {
        return;
    }
    if let Some(root) = std::env::var_os("TERMED_ROUTER_SETUP_ROOT") {
        let root = PathBuf::from(root);
        let mut a = app(&root);
        a.run(Command::AgentSettings);
        choose(&mut a, "Step budget:");
        type_value(&mut a, "7");
        choose(&mut a, "Save Router API Key");
        type_value(&mut a, "test-only-saved+key");
        assert!(a.agent.options.uses_router());
        assert_eq!(a.agent.options.backend, "builtin");
        assert!(a.agent.options.model.is_empty()); // resolves to free:code
        assert_eq!(a.agent.options.max_steps, 7);
        let settings = std::fs::read_to_string(user_settings_path()).unwrap();
        assert!(!settings.contains("test-only-saved+key"));
        assert!(!settings.contains("router_api_key"));
        let saved = termed::credentials::Store::default()
            .load()
            .unwrap()
            .unwrap();
        assert_eq!(saved.expose(), "test-only-saved+key");
        let override_set = std::env::var_os("TERMED_ROUTER_SETUP_OVERRIDE").is_some();
        let selected =
            termed::provider::authorization(&a.agent.options, termed::provider::ROUTER_ENDPOINT)
                .unwrap()
                .unwrap();
        assert_eq!(
            selected.expose(),
            if override_set {
                "test-only-override+key"
            } else {
                "test-only-saved+key"
            }
        );
        assert!(a.toasts.iter().any(|toast| if override_set {
            toast.text.contains("environment override")
        } else {
            toast.text.contains("NoKey Router selected")
        }));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(termed::credentials::path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        // Replacing a key must not reset a router model already chosen.
        let mut prefs = a.preferences.clone();
        prefs.agent.model = "free:fast".into();
        a.save_preferences(prefs).unwrap();
        a.run(Command::AgentSettings);
        choose(&mut a, "Save Router API Key");
        type_value(&mut a, "test-only-replacement+key");
        assert_eq!(a.agent.options.model, "free:fast");
        assert_eq!(app(&root).agent.options.model, "free:fast");

        // --router is a launch-only override. Saving a key while it is active
        // must also persist the selected router and model for the next launch.
        let transient = root.join("transient");
        std::fs::create_dir_all(&transient).unwrap();
        let mut c = app(&transient);
        c.preferences.agent.use_router();
        c.preferences.agent.model = "free:smart".into();
        c.agent
            .configure(c.preferences.agent.clone(), String::new())
            .unwrap();
        assert!(!settings_path(&transient).exists());
        c.run(Command::AgentSettings);
        choose(&mut c, "Save Router API Key");
        type_value(&mut c, "test-only-transient+key");
        let reloaded = app(&transient);
        assert!(reloaded.agent.options.uses_router());
        assert_eq!(reloaded.agent.options.model, "free:smart");

        // A broken settings file cannot be overwritten, but a successful key
        // save must be reported honestly and the masked input cleared.
        let broken = root.join("broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(user_settings_path(), "invalid JSON").unwrap();
        let mut b = app(&broken);
        assert!(b.management.load_error.is_some());
        b.run(Command::AgentSettings);
        choose(&mut b, "Save Router API Key");
        type_value(&mut b, "test-only-broken-settings+key");
        assert!(b.management.edit.is_none());
        assert!(
            b.toasts
                .iter()
                .any(|t| t.text.contains("key saved locally")
                    && t.text.contains("could not be saved"))
        );
        assert_eq!(
            std::fs::read_to_string(user_settings_path()).unwrap(),
            "invalid JSON"
        );
        assert_eq!(
            termed::credentials::Store::default()
                .load()
                .unwrap()
                .unwrap()
                .expose(),
            "test-only-broken-settings+key"
        );
        return;
    }

    for environment_override in [false, true] {
        let root = project(if environment_override {
            "router-setup-env"
        } else {
            "router-setup-saved"
        });
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "saving_router_key_selects_builtin_and_reports_override_or_selection_failure",
            ])
            .env("TERMED_ROUTER_SETUP_ROOT", &root)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env_remove("ROUTER_API_KEY");
        if environment_override {
            command
                .env("TERMED_ROUTER_SETUP_OVERRIDE", "1")
                .env("ROUTER_API_KEY", "test-only-override+key");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn builtin_router_is_default_even_when_opencode_is_on_path_or_in_environment() {
    if common::isolate("builtin_router_is_default_even_when_opencode_is_on_path_or_in_environment")
    {
        return;
    }
    if let Some(root) = std::env::var_os("TERMED_DEFAULT_ROUTER_CHILD") {
        let root = PathBuf::from(root);
        let mut a = app(&root);
        assert_eq!(a.agent.options.backend, "builtin");
        assert!(a.agent.options.uses_router());
        assert_eq!(
            termed::provider::endpoint(&a.agent.options),
            termed::provider::ROUTER_ENDPOINT
        );
        assert!(a.agent.spec.as_ref().unwrap().builtin);
        assert!(
            a.agent
                .available
                .iter()
                .all(|agent| !agent.name.eq_ignore_ascii_case("opencode"))
        );
        a.run(Command::AgentSettings);
        let items = &a.palette.as_ref().unwrap().items;
        assert!(
            items
                .iter()
                .any(|item| item.contains(termed::provider::ROUTER_ENDPOINT))
        );
        assert!(
            items
                .iter()
                .any(|item| item.contains(termed::provider::ROUTER_MODEL))
        );
        assert!(!user_settings_path().exists());
        return;
    }
    let root = project("router-default");
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("opencode"), "not an agent").unwrap();
    let mut dirs = vec![bin];
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "builtin_router_is_default_even_when_opencode_is_on_path_or_in_environment",
        ])
        .env("TERMED_DEFAULT_ROUTER_CHILD", &root)
        .env("TERMED_AGENT", "opencode acp")
        .env("PATH", std::env::join_paths(dirs).unwrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn legacy_opencode_settings_migrate_without_overwriting_workspace_files() {
    if common::isolate("legacy_opencode_settings_migrate_without_overwriting_workspace_files") {
        return;
    }
    let root = project("legacy-opencode");
    std::fs::create_dir_all(root.join(".termed")).unwrap();
    let old = r#"{"agent":{"backend":"OpenCode","provider":"auto","model":"opencode/big-pickle","read_only":true,"instructions":"Keep existing rules"},"editor":{"minimap":false}}"#;
    std::fs::write(settings_path(&root), old).unwrap();
    let mut a = app(&root);
    assert_eq!(a.agent.options.backend, "builtin");
    assert!(a.agent.options.uses_router());
    assert_eq!(
        termed::provider::endpoint(&a.agent.options),
        termed::provider::ROUTER_ENDPOINT
    );
    assert!(a.agent.options.model.is_empty());
    assert!(a.agent.options.read_only);
    assert_eq!(a.agent.options.instructions, "Keep existing rules");
    assert!(!a.settings.minimap);
    assert!(
        !user_settings_path().exists(),
        "loading must not silently write files"
    );
    a.save_preferences(a.preferences.clone()).unwrap();
    assert!(user_settings_path().exists());
    assert_eq!(std::fs::read_to_string(settings_path(&root)).unwrap(), old);

    let other = project("legacy-next-folder");
    let b = app(&other);
    assert_eq!(b.agent.options.backend, "builtin");
    assert!(b.agent.options.uses_router() && b.agent.options.read_only);
    assert_eq!(b.agent.options.instructions, "Keep existing rules");
    assert!(!b.settings.minimap);
}

#[test]
fn legacy_auto_backend_and_custom_opencode_commands_migrate() {
    if common::isolate("legacy_auto_backend_and_custom_opencode_commands_migrate") {
        return;
    }
    for (backend, command) in [
        ("auto", ""),
        ("custom", "opencode acp"),
        ("custom", "NPX --yes opencode-ai@latest acp"),
    ] {
        let root = project("old-backend");
        std::fs::create_dir_all(root.join(".termed")).unwrap();
        std::fs::write(settings_path(&root), serde_json::json!({
            "agent": {"backend": backend, "command": command, "provider": "auto", "model": "opencode/big-pickle"}
        }).to_string()).unwrap();
        let a = app(&root);
        assert_eq!(a.agent.options.backend, "builtin");
        assert!(a.agent.options.uses_router());
        assert!(a.agent.options.command.is_empty());
        assert!(a.agent.options.model.is_empty());
        assert!(a.agent.spec.as_ref().unwrap().builtin);
    }
}

#[test]
fn partial_legacy_custom_provider_and_subagent_inheritance_are_preserved() {
    if common::isolate("partial_legacy_custom_provider_and_subagent_inheritance_are_preserved") {
        return;
    }
    let root = project("legacy-custom");
    std::fs::create_dir_all(root.join(".termed")).unwrap();
    std::fs::write(settings_path(&root), r#"{
      "agent": {"backend": "builtin", "endpoint": "https://custom.invalid/v1", "api_key_env": "CUSTOM_KEY", "model": "custom-model", "read_only": true},
      "sub_agents": [{"id": "reviewer", "name": "Reviewer", "agent": {"instructions": "Review only"}}]
    }"#).unwrap();
    let prefs = Preferences::load(&root).unwrap();
    assert_eq!(prefs.agent.provider, "auto");
    assert_eq!(prefs.agent.endpoint, "https://custom.invalid/v1");
    let worker = &prefs.sub_agents[0].agent;
    assert_eq!(worker.provider, "auto");
    let inherited = worker.restricted_by(&prefs.agent);
    assert_eq!(inherited.endpoint, "https://custom.invalid/v1");
    assert_eq!(inherited.api_key_env, "CUSTOM_KEY");
    assert_eq!(inherited.model, "custom-model");
    assert!(inherited.read_only);
    inherited.validate().unwrap();
}

#[test]
fn custom_endpoint_selection_does_not_forward_router_credentials() {
    if common::isolate("custom_endpoint_selection_does_not_forward_router_credentials") {
        return;
    }
    let root = project("custom-endpoint");
    let mut a = app(&root);
    let mut prefs = a.preferences.clone();
    prefs.agent.api_key_env = "ROUTER_API_KEY".into();
    a.save_preferences(prefs).unwrap();
    a.run(Command::AgentSettings);
    choose(&mut a, "API base URL:");
    type_value(&mut a, "https://another-provider.invalid/v1");
    assert_eq!(a.agent.options.provider, "custom");
    assert!(a.agent.options.api_key_env.is_empty());
    assert!(!a.agent.options.uses_router());
    assert!(
        termed::provider::authorization(&a.agent.options, "https://another-provider.invalid/v1")
            .unwrap()
            .is_none()
    );
    let b = app(&project("custom-next-folder"));
    assert_eq!(b.agent.options.provider, "custom");
    assert!(b.agent.options.api_key_env.is_empty());
}

#[test]
fn settings_replace_failure_is_reported_and_temp_file_removed() {
    if common::isolate("settings_replace_failure_is_reported_and_temp_file_removed") {
        return;
    }
    let root = project("save-failure");
    let path = user_settings_path();
    std::fs::create_dir_all(&path).unwrap(); // A directory cannot be replaced by a file.
    let error = Preferences::default().save(&root).unwrap_err().to_string();
    assert!(error.contains("Cannot replace user settings"));
    assert!(path.is_dir());
    let entries: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().collect();
    assert_eq!(entries.len(), 1, "temporary file was left behind");
    assert!(!settings_path(&root).exists());
}

#[test]
fn relative_or_empty_xdg_config_home_cannot_make_settings_project_local() {
    if common::isolate("relative_or_empty_xdg_config_home_cannot_make_settings_project_local") {
        return;
    }
    if let Some(root) = std::env::var_os("TERMED_RELATIVE_CONFIG_CHILD") {
        let root = PathBuf::from(root);
        let first = root.join("first");
        let second = root.join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::env::set_current_dir(&first).unwrap();
        let expected = root.join("home/.config/termed/settings.json");
        assert_eq!(user_settings_path(), expected);
        let mut prefs = Preferences::default();
        prefs.editor.minimap = false;
        prefs.save(&first).unwrap();
        std::env::set_current_dir(&second).unwrap();
        assert_eq!(user_settings_path(), expected);
        assert!(!Preferences::load(&second).unwrap().editor.minimap);
        assert!(!second.join("relative-xdg/termed/settings.json").exists());
        return;
    }
    for value in ["", "relative-xdg"] {
        let root = project("relative-config");
        std::fs::create_dir_all(root.join("home")).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "relative_or_empty_xdg_config_home_cannot_make_settings_project_local",
            ])
            .env("TERMED_RELATIVE_CONFIG_CHILD", &root)
            .env("XDG_CONFIG_HOME", value)
            .env("HOME", root.join("home"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn legacy_builtin_implicit_gateway_becomes_router_but_new_explicit_auto_is_preserved() {
    if common::isolate(
        "legacy_builtin_implicit_gateway_becomes_router_but_new_explicit_auto_is_preserved",
    ) {
        return;
    }
    let root = project("legacy-builtin");
    let path = user_settings_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let old = r#"{"version":1,"agent":{"backend":"builtin","provider":"auto","model":"opencode/big-pickle","read_only":true}}"#;
    std::fs::write(&path, old).unwrap();
    let mut prefs = Preferences::load(&root).unwrap();
    assert_eq!(prefs.version, 2);
    assert!(prefs.agent.uses_router());
    assert!(prefs.agent.model.is_empty());
    assert!(prefs.agent.read_only);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), old);
    prefs.save(&root).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap()["version"],
        2
    );

    // A deliberate choice of legacy auto in the new schema stays a choice,
    // rather than being mistaken for the old implicit default every launch.
    prefs.agent.provider = "auto".into();
    prefs.save(&root).unwrap();
    let selected = Preferences::load(&project("explicit-auto-other-folder")).unwrap();
    assert_eq!(selected.agent.provider, "auto");
    assert!(!selected.agent.uses_router());
    assert!(selected.agent.read_only);
}

#[test]
fn choosing_explicit_custom_acp_command_persists_without_the_environment() {
    if common::isolate("choosing_explicit_custom_acp_command_persists_without_the_environment") {
        return;
    }
    const COMMAND: &str = "termed-test-uninstalled-agent --flag 'value with spaces'";
    if let Some(phase) = std::env::var_os("TERMED_CUSTOM_ACP_PHASE") {
        let root = PathBuf::from(std::env::var_os("TERMED_CUSTOM_ACP_ROOT").unwrap());
        let mut a = app(&root);
        if phase == "select" {
            a.run(Command::AgentPick);
            choose(&mut a, "termed-test-uninstalled-agent");
            assert_eq!(a.preferences.agent.backend, "custom");
            assert_eq!(a.preferences.agent.command, COMMAND);
        } else {
            assert_eq!(a.agent.options.backend, "custom");
            assert_eq!(a.agent.options.command, COMMAND);
            let spec = a.agent.spec.as_ref().unwrap();
            assert_eq!(spec.program, "termed-test-uninstalled-agent");
            assert_eq!(spec.args, vec!["--flag", "value with spaces"]);
        }
        return;
    }
    let root = project("custom-agent-persist");
    for phase in ["select", "reload"] {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "choosing_explicit_custom_acp_command_persists_without_the_environment",
            ])
            .env("TERMED_CUSTOM_ACP_PHASE", phase)
            .env("TERMED_CUSTOM_ACP_ROOT", &root)
            .env_remove("TERMED_AGENT");
        if phase == "select" {
            command.env("TERMED_AGENT", COMMAND);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn dirty_legacy_settings_are_not_silently_migrated() {
    if common::isolate("dirty_legacy_settings_are_not_silently_migrated") {
        return;
    }
    let root = project("dirty-legacy");
    let path = settings_path(&root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = r#"{"agent":{"read_only":true}}"#;
    std::fs::write(&path, original).unwrap();
    let mut a = app(&root);
    a.open_file(&path);
    a.editor_mut().unwrap().insert_text(" ");
    let mut changed = a.preferences.clone();
    changed.editor.minimap = false;
    assert!(a.save_preferences(changed).is_err());
    assert!(a.editor().unwrap().doc.is_dirty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    assert!(!user_settings_path().exists());
}
