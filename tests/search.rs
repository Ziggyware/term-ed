//! Find/Replace in Files: the glob filter language, project search and
//! replacement, plus the panel driven through the real key handler.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use termed::app::{App, Focus, SearchField};
use termed::commands::Command;
use termed::search::{FilterSet, Query, TextFile, replace_in_text, search};

fn project(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("termed-search-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let files = [
        (
            "src/Program.cs",
            "class Program {\n    // Console app\n    static void Main() { Console.WriteLine(\"hi\"); }\n}\n",
        ),
        ("src/Widget.cs", "class Widget { }\n"),
        ("src/App.csproj", "<Project>class</Project>\n"),
        ("src/bin/Debug/Generated.cs", "class Generated { }\n"),
        ("src/nested/bin/Deep.cs", "class Deep { }\n"),
        ("tests/ProgramTests.cs", "class ProgramTests { }\n"),
        ("README.md", "A class project\n"),
    ];
    for (rel, text) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    app.handle_event(Event::Key(KeyEvent::new(code, mods)));
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c), KeyModifiers::NONE);
    }
}

fn screen(term: &Terminal<TestBackend>) -> String {
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn rels(app: &App) -> Vec<String> {
    app.search
        .results
        .files
        .iter()
        .map(|f| f.rel.clone())
        .collect()
}

#[test]
fn query_options_build_the_expected_regex() {
    let q = Query {
        text: "CONSOLE".into(),
        ..Default::default()
    };
    assert!(q.compile().unwrap().is_match("Console.WriteLine"));

    let q = Query {
        text: "class".into(),
        whole_word: true,
        ..Default::default()
    };
    let re = q.compile().unwrap();
    assert!(re.is_match("a class here"));
    assert!(!re.is_match("classy"));

    let q = Query {
        text: r"Console\.\w+".into(),
        regex: true,
        ..Default::default()
    };
    assert!(q.compile().unwrap().is_match("Console.WriteLine"));
    // Literal mode escapes regex punctuation.
    let q = Query::new("Console.WriteLine");
    assert!(!q.compile().unwrap().is_match("ConsolexWriteLine"));
}

#[test]
fn search_uses_gitignore_style_walk_and_the_glob_filter() {
    let root = project("filter");
    let filter = FilterSet::parse(r"*.cs*;!*.csproj;!bin\");
    let results = search(&root, &Query::new("class"), &filter, &HashMap::new()).unwrap();
    let found: Vec<&str> = results.files.iter().map(|f| f.rel.as_str()).collect();
    assert_eq!(
        found,
        vec!["src/Program.cs", "src/Widget.cs", "tests/ProgramTests.cs"]
    );
    assert_eq!(results.matches, 3);
    assert!(!results.truncated);

    // Without the filter, everything text-like is searched, including bin/.
    let all = search(
        &root,
        &Query::new("class"),
        &FilterSet::default(),
        &HashMap::new(),
    )
    .unwrap();
    let all_names: Vec<&str> = all.files.iter().map(|f| f.rel.as_str()).collect();
    assert!(all_names.contains(&"src/bin/Debug/Generated.cs"));
    assert!(all_names.contains(&"src/nested/bin/Deep.cs"));
    assert!(all_names.contains(&"src/App.csproj"));

    // Line and column details drive "jump to hit".
    let program = results
        .files
        .iter()
        .find(|f| f.rel == "src/Program.cs")
        .unwrap();
    let hit = &program.matches[0];
    assert_eq!(hit.line, 0);
    assert_eq!(hit.start, 0);
    assert_eq!(hit.end, 5);
    assert_eq!(&hit.text[hit.start..hit.end], "class");
}

#[test]
fn unsaved_editor_text_is_searched() {
    let root = project("overrides");
    let path = root.join("src/Widget.cs");
    let mut overrides = HashMap::new();
    overrides.insert(path.clone(), "class Renamed\n".to_string());
    let results = search(
        &root,
        &Query::new("Renamed"),
        &FilterSet::parse("*.cs"),
        &overrides,
    )
    .unwrap();
    assert_eq!(results.files.len(), 1);
    assert_eq!(results.files[0].rel, "src/Widget.cs");
    assert_eq!(results.matches, 1);
    assert_eq!(results.files[0].matches[0].start, 6);
}

#[test]
fn replace_in_text_keeps_crlf_when_writing() {
    let root = project("crlf");
    let path = root.join("src/Crlf.cs");
    std::fs::write(&path, "class A { }\r\nclass B { }\r\n").unwrap();
    let file = TextFile::read(&path).unwrap();
    assert!(file.crlf);
    assert!(!file.text.contains('\r'));
    let (new, n) = replace_in_text(
        &file.text,
        &Query::new("class").compile().unwrap(),
        "struct",
        true,
    );
    assert_eq!(n, 2);
    file.write(&new).unwrap();
    let disk = std::fs::read_to_string(&path).unwrap();
    assert_eq!(disk, "struct A { }\r\nstruct B { }\r\n");
}

#[test]
fn panel_searches_filters_opens_and_replaces() {
    let root = project("app");
    let mut app = App::new(
        root.clone(),
        vec![root.join("src/Program.cs")],
        "Visual Studio Dark",
        false,
    );
    app.run(Command::FindInFiles);
    assert!(app.search.visible);
    assert_eq!(app.focus, Focus::Search);

    type_text(&mut app, "class");
    assert_eq!(app.focus, Focus::Search);
    assert_eq!(app.search.query, "class");
    assert!(app.search.results.matches >= 3);

    // Apply the filter from the example and make sure bin/ and .csproj drop out.
    app.search.field = SearchField::Filter;
    type_text(&mut app, r"*.cs*;!*.csproj;!bin\");
    assert_eq!(app.search.filter, r"*.cs*;!*.csproj;!bin\");
    let names = rels(&app);
    assert_eq!(
        names,
        vec!["src/Program.cs", "src/Widget.cs", "tests/ProgramTests.cs"]
    );

    // Down moves through the hits; Enter opens the selected one.
    key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(app.search.selected, Some((1, 0)));
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let e = app.editor().unwrap();
    assert_eq!(e.doc.name(), "Widget.cs");
    assert_eq!(e.selected_text(), "class");

    // Replace All: the open file changes in memory (dirty, undoable), the
    // others are written to disk, and the filter is honored.
    app.search.replacement = "struct".to_string();
    app.run(Command::ReplaceInFiles);
    assert!(app.search.replace);
    key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);

    let open = app
        .editors
        .iter()
        .find(|e| e.doc.name() == "Widget.cs")
        .unwrap();
    assert!(open.doc.is_dirty());
    assert!(open.doc.rope.to_string().contains("struct Widget"));
    assert_eq!(
        std::fs::read_to_string(root.join("src/Widget.cs")).unwrap(),
        "class Widget { }\n"
    );

    assert!(
        std::fs::read_to_string(root.join("tests/ProgramTests.cs"))
            .unwrap()
            .contains("struct ProgramTests")
    );
    assert!(
        std::fs::read_to_string(root.join("src/Program.cs"))
            .unwrap()
            .contains("class Program")
    );
    assert!(
        std::fs::read_to_string(root.join("src/App.csproj"))
            .unwrap()
            .contains("class")
    );
    assert!(
        std::fs::read_to_string(root.join("src/bin/Debug/Generated.cs"))
            .unwrap()
            .contains("class")
    );
}

#[test]
fn revert_undoes_an_in_memory_replace() {
    let root = project("undo");
    let mut app = App::new(
        root.clone(),
        vec![root.join("src/Program.cs")],
        "Visual Studio Dark",
        false,
    );
    app.run(Command::ReplaceInFiles);
    // Scope the search to the open file so the first hit is in its buffer.
    app.search.field = SearchField::Filter;
    type_text(&mut app, "src/Program.cs");
    app.search.field = SearchField::Query;
    type_text(&mut app, "class");
    assert_eq!(app.search.results.files.len(), 1);
    assert_eq!(app.search.selected, Some((0, 0)));
    app.search.replacement = "struct".to_string();
    // Ctrl+Shift+Enter replaces just the selected hit, in memory.
    key(
        &mut app,
        KeyCode::Enter,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert!(
        app.editor()
            .unwrap()
            .doc
            .rope
            .to_string()
            .contains("struct Program")
    );
    app.run(Command::Undo);
    assert!(
        app.editor()
            .unwrap()
            .doc
            .rope
            .to_string()
            .contains("class Program")
    );
}

#[test]
fn panel_renders_title_results_and_status() {
    let root = project("render");
    let mut app = App::new(
        root.clone(),
        vec![root.join("src/Program.cs")],
        "Visual Studio Dark",
        false,
    );
    app.run(Command::FindInFiles);
    type_text(&mut app, "Console");
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| termed::ui::draw(f, &mut app, true)).unwrap();
    let screen = screen(&term);
    assert!(screen.contains("Find in Files"), "{screen}");
    assert!(screen.contains("Program.cs"), "{screen}");
    assert!(screen.contains("2 matches in 1 file"), "{screen}");
}

#[test]
fn panel_fits_the_compact_phone_layout() {
    let root = project("compact");
    let mut app = App::new(
        root.clone(),
        vec![root.join("src/Program.cs")],
        "Visual Studio Dark",
        false,
    );
    app.run(Command::FindInFiles);
    type_text(&mut app, "Console");
    // Narrower than 80 columns: the UI switches to the compact layout.
    let mut term = Terminal::new(TestBackend::new(60, 24)).unwrap();
    term.draw(|f| termed::ui::draw(f, &mut app, true)).unwrap();
    let screen = screen(&term);
    assert!(screen.contains("Find in Files"), "{screen}");
    assert!(screen.contains("2 matches in 1 file"), "{screen}");
    assert!(app.layout.search_box.width <= 60);
}

#[test]
fn root_and_directory_boxes_scope_the_search() {
    let root = project("scope");
    let mut app = App::new(
        root.clone(),
        vec![root.join("src/Program.cs")],
        "Visual Studio Dark",
        false,
    );
    app.run(Command::FindInFiles);
    type_text(&mut app, "class");
    assert!(app.search.results.matches >= 3);

    // Directory box: skip generated folders, and only inside src.
    app.search.field = SearchField::Dirs;
    type_text(&mut app, "!bin;!obj");
    let names = rels(&app);
    assert!(!names.iter().any(|n| n.contains("bin/")), "{names:?}");

    // Root box: a sub-folder becomes the search root. Paths in the results
    // are relative to the root that was searched.
    app.search.field = SearchField::Root;
    type_text(&mut app, "src");
    let names = rels(&app);
    assert!(names.contains(&"Program.cs".to_string()), "{names:?}");
    assert!(!names.iter().any(|n| n.starts_with("tests/")), "{names:?}");
    assert!(names.iter().all(|n| !n.contains("/bin/")), "{names:?}");
    assert_eq!(app.search.root_path, root.join("src"));
    assert!(!app.search.missing_root);

    // A root that does not exist is reported instead of searching nothing.
    app.search.root.clear();
    app.search.root_cursor = 0;
    type_text(&mut app, "no-such-folder");
    assert!(app.search.missing_root);
    assert_eq!(app.search.results.matches, 0);

    // Back to the project root: everything is searched again.
    app.search.root.clear();
    app.search.root_cursor = 0;
    app.search.dirs.clear();
    app.search.dirs_cursor = 0;
    app.search.field = SearchField::Query;
    app.search.query.clear();
    app.search.query_cursor = 0;
    app.search.query_selected = false;
    type_text(&mut app, "class");
    assert!(!app.search.missing_root);
    assert!(app.search.results.matches >= 3);
}

#[test]
fn panel_renders_the_root_and_filter_rows() {
    let root = project("rows");
    let mut app = App::new(
        root.clone(),
        vec![root.join("src/Program.cs")],
        "Visual Studio Dark",
        false,
    );
    app.run(Command::FindInFiles);
    type_text(&mut app, "class");
    app.search.dirs = "!bin".into();
    app.search.filter = "*.cs;!*.csproj".into();
    let mut term = Terminal::new(TestBackend::new(110, 32)).unwrap();
    term.draw(|f| termed::ui::draw(f, &mut app, true)).unwrap();
    let screen = screen(&term);
    assert!(screen.contains("!bin"), "{screen}");
    assert!(screen.contains("*.cs;!*.csproj"), "{screen}");
    // Four hit-testable fields: query, root, dirs, files.
    let ids: Vec<u8> = app.layout.search_fields.iter().map(|(_, id)| *id).collect();
    assert_eq!(ids, vec![0, 2, 3, 4]);
}

#[test]
fn directory_scopes_reach_nested_and_wildcard_paths() {
    let root = project("nested-glob");
    let nested = root.join("packages/app/src/deep");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("Nested.cs"), "class Nested {}\n").unwrap();
    for dirs in ["src", "**/src", "packages/*/src", "packages/**/src"] {
        let results = search(
            &root,
            &Query::new("class"),
            &FilterSet::parse_parts(dirs, "*.cs"),
            &HashMap::new(),
        )
        .unwrap();
        assert!(
            results
                .files
                .iter()
                .any(|f| f.rel == "packages/app/src/deep/Nested.cs"),
            "scope {dirs}: {results:?}"
        );
    }
    let results = search(
        &root,
        &Query::new("class"),
        &FilterSet::parse_parts("src;!deep", "*.cs"),
        &HashMap::new(),
    )
    .unwrap();
    assert!(!results.files.iter().any(|f| f.rel.ends_with("Nested.cs")));
}

#[test]
fn open_buffers_include_ignored_and_not_yet_created_files_without_duplicates() {
    let root = project("new-ignored-buffers");
    std::fs::write(root.join(".gitignore"), "ignored.cs\n").unwrap();
    std::fs::write(root.join("ignored.cs"), "old disk content\n").unwrap();
    let buffers = HashMap::from([
        (
            root.join("ignored.cs"),
            "needle from ignored buffer\n".into(),
        ),
        (root.join("new.cs"), "needle from new buffer\n".into()),
        (
            root.join("src/../src/Widget.cs"),
            "needle from normalized buffer\n".into(),
        ),
        (
            root.parent().unwrap().join("outside.cs"),
            "needle outside project\n".into(),
        ),
    ]);
    let result = search(
        &root.join("src/.."),
        &Query::new("needle"),
        &FilterSet::parse("*.cs"),
        &buffers,
    )
    .unwrap();
    assert_eq!(result.matches, 3);
    assert_eq!(
        result
            .files
            .iter()
            .map(|f| f.rel.as_str())
            .collect::<Vec<_>>(),
        vec!["ignored.cs", "new.cs", "src/Widget.cs"]
    );
    let result = search(
        &root,
        &Query::new("needle"),
        &FilterSet::parse("*.cs;!ignored.cs"),
        &buffers,
    )
    .unwrap();
    assert_eq!(
        result.matches, 2,
        "open buffers must still honor explicit filters"
    );
}

#[test]
fn pasted_query_replaces_the_prefilled_selection() {
    let root = project("paste-selected");
    let mut app = App::new(
        root.clone(),
        vec![root.join("src/Program.cs")],
        "Visual Studio Dark",
        false,
    );
    app.editor_mut().unwrap().sel = termed::document::Selection { anchor: 0, head: 5 };
    app.run(Command::FindInFiles);
    assert!(app.search.query_selected);
    app.handle_event(Event::Paste("Console".into()));
    assert_eq!(app.search.query, "Console");
    assert_eq!(app.search.results.matches, 2);
}

#[test]
fn search_from_the_phone_agent_sheet_is_visible() {
    let root = project("phone-agent");
    let mut app = App::new(
        root.clone(),
        vec![root.join("src/Program.cs")],
        "Visual Studio Dark",
        false,
    );
    app.set_compact(true);
    app.run(Command::AgentToggle);
    app.run(Command::FindInFiles);
    assert!(!app.show_agent);
    type_text(&mut app, "Console");
    let mut term = Terminal::new(TestBackend::new(46, 30)).unwrap();
    term.draw(|f| termed::ui::draw(f, &mut app, true)).unwrap();
    assert!(screen(&term).contains("Find in Files"));
}

#[test]
fn unreadable_roots_report_an_error_instead_of_a_clean_empty_result() {
    let root = project("missing-root").join("not-there");
    let result = search(
        &root,
        &Query::new("class"),
        &FilterSet::default(),
        &HashMap::new(),
    )
    .unwrap();
    assert!(!result.warnings.is_empty());
}
