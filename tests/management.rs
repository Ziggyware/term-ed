use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use termed::app::{App, PaletteMode};
use termed::commands::Command;
use termed::config::{AgentConfig, CommandPolicy, Preferences, settings_path};

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
    assert!(settings_path(&root).is_file());
    // Invalid edits don't corrupt the previous saved value.
    choose(&mut a, "Step budget:");
    type_value(&mut a, "0");
    assert_eq!(a.palette.as_ref().unwrap().mode, PaletteMode::Input);
    assert_eq!(Preferences::load(&root).unwrap().agent.max_steps, 9);
}

#[test]
fn invalid_json_is_preserved_and_reported() {
    let root = project("invalid");
    std::fs::create_dir_all(root.join(".termed")).unwrap();
    std::fs::write(settings_path(&root), "{ invalid").unwrap();
    let mut a = app(&root);
    assert!(a.management.load_error.is_some());
    assert!(a.save_preferences(Preferences::default()).is_err());
    assert_eq!(
        std::fs::read_to_string(settings_path(&root)).unwrap(),
        "{ invalid"
    );
    std::fs::write(
        settings_path(&root),
        r#"{"agent":{"backend":"builtin","read_only":true}}"#,
    )
    .unwrap();
    a.reload_preferences();
    assert!(a.management.load_error.is_none());
    assert!(a.agent.options.read_only);
}

#[test]
fn sub_agent_profiles_can_be_created_edited_disabled_and_removed() {
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
    let parent = AgentConfig {
        endpoint: "https://one.invalid/v1".into(),
        api_key_env: "ONE_KEY".into(),
        max_steps: 7,
        task_timeout_secs: 45,
        ..Default::default()
    };
    let child = AgentConfig {
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
        AgentConfig::default().restricted_by(&parent).api_key_env,
        "ONE_KEY"
    );
}

#[cfg(unix)]
#[test]
fn workspace_paths_reject_symlink_escapes_even_for_new_files() {
    let root = project("symlink-root");
    let outside = project("symlink-outside");
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    assert!(termed::config::workspace_path(&root, "link/new.txt").is_err());
    assert!(termed::config::workspace_path(&root, "../outside.txt").is_err());
    assert!(termed::config::workspace_path(&root, "new/file.txt").is_ok());
}

#[test]
fn model_picker_does_not_overwrite_unsaved_settings_json() {
    let root = project("model-dirty-settings");
    let mut prefs = Preferences::default();
    prefs.agent.backend = "builtin".into();
    prefs.agent.model = "old-model".into();
    prefs.save(&root).unwrap();
    let original = std::fs::read_to_string(settings_path(&root)).unwrap();
    let mut a = app(&root);
    a.open_file(&settings_path(&root));
    a.editor_mut().unwrap().insert_text(" ");
    a.agent.models = vec![("new-model".into(), "New model".into())];
    a.run(Command::AgentModel);
    choose(&mut a, "new-model");
    assert_eq!(a.preferences.agent.model, "old-model");
    assert_eq!(
        std::fs::read_to_string(settings_path(&root)).unwrap(),
        original
    );
    assert!(a.editor().unwrap().doc.is_dirty());
}

#[cfg(unix)]
#[test]
fn settings_menu_preserves_dirty_json_opened_through_a_directory_symlink() {
    let root = project("settings-symlink");
    let target = root.join("configuration");
    std::fs::create_dir_all(&target).unwrap();
    std::os::unix::fs::symlink(&target, root.join(".termed")).unwrap();
    let mut prefs = Preferences::default();
    prefs.save(&root).unwrap();
    let original = std::fs::read_to_string(settings_path(&root)).unwrap();
    let mut a = app(&root);
    a.open_file(&settings_path(&root));
    a.editor_mut().unwrap().insert_text(" ");
    prefs.editor.minimap = false;
    assert!(a.save_preferences(prefs).is_err());
    assert_eq!(
        std::fs::read_to_string(settings_path(&root)).unwrap(),
        original
    );
    assert!(a.editor().unwrap().doc.is_dirty());
}

#[cfg(unix)]
#[test]
fn skill_aliases_cannot_bypass_unsaved_instruction_protection() {
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
    let settings = std::fs::read_to_string(settings_path(&root)).unwrap();
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
        assert_eq!(a.agent.options.model, "free:code");
        assert_eq!(a.agent.options.max_steps, 7);
        let settings = std::fs::read_to_string(settings_path(&root)).unwrap();
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
        std::fs::create_dir_all(broken.join(".termed")).unwrap();
        std::fs::write(settings_path(&broken), "invalid JSON").unwrap();
        let mut b = app(&broken);
        b.run(Command::AgentSettings);
        choose(&mut b, "Save Router API Key");
        type_value(&mut b, "test-only-broken-settings+key");
        assert!(!b.agent.options.uses_router());
        assert!(b.management.edit.is_none());
        assert!(
            b.toasts
                .iter()
                .any(|t| t.text.contains("key saved locally")
                    && t.text.contains("could not be saved"))
        );
        assert_eq!(
            std::fs::read_to_string(settings_path(&broken)).unwrap(),
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
