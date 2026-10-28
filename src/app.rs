//! Application state and input handling.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use regex::{Regex, RegexBuilder};
use serde_json::{Value, json};

use crate::agent::{self, Agent, State as AgentState};
use crate::automation::{Automation, Source};
use crate::clipboard::Clipboard;
use crate::commands::{COMMANDS, Command};
use crate::config::Preferences;
use crate::document::{Document, Indent, LineEnding, Selection};
use crate::editor::Editor;
use crate::explorer::{self, Explorer};
use crate::fuzzy;
use crate::highlight;
use crate::management::Management;
use crate::plugin::{self, Host as PluginHost};
use crate::search;
use crate::skills::Skill;
use crate::theme::{self, ThemeEntry};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Explorer,
    Find,
    /// Typing into the find/replace-in-files panel.
    Search,
    /// Typing into the AI agent panel.
    Agent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteMode {
    Commands,
    Files,
    GotoLine,
    Themes,
    Languages,
    SaveAs,
    AgentModels,
    AgentPicker,
    /// Manage plugins: reload, reinstall the bundled ones, enable/disable.
    Plugins,
    Settings,
    AgentSettings,
    SubAgents,
    SubAgentSettings,
    Skills,
    SkillActions,
    PluginActions,
    PluginCommands,
    Input,
    ConfirmRemoveSkill,
    ConfirmRemoveSubAgent,
}

pub struct Palette {
    pub mode: PaletteMode,
    pub input: String,
    pub cursor: usize,
    pub input_selected: bool,
    pub items: Vec<String>,
    pub hints: Vec<String>,
    pub filtered: Vec<fuzzy::Scored>,
    pub selected: usize,
    pub scroll: usize,
    /// Theme active before a live-preview picker opened (restored on Esc).
    pub restore_theme: Option<String>,
    /// Plugin commands appended to the command list, as (plugin, command).
    pub plugin_cmds: Vec<(usize, usize)>,
}

/// A sticky modifier for touch screens: soft keyboards cannot hold Ctrl,
/// Alt or Shift while pressing another key, so a tap latches the modifier.
/// Tap once: applies to the next key. Tap again: locked until tapped off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Latch {
    #[default]
    Off,
    Once,
    Locked,
}

impl Latch {
    pub fn on(self) -> bool {
        self != Latch::Off
    }
    fn tapped(self) -> Self {
        match self {
            Latch::Off => Latch::Once,
            Latch::Once => Latch::Locked,
            Latch::Locked => Latch::Off,
        }
    }
    fn spent(self) -> Self {
        if self == Latch::Once {
            Latch::Off
        } else {
            self
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mod {
    Ctrl,
    Alt,
    Shift,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StickyMods {
    pub ctrl: Latch,
    pub alt: Latch,
    pub shift: Latch,
}

impl StickyMods {
    pub fn get(&self, m: Mod) -> Latch {
        match m {
            Mod::Ctrl => self.ctrl,
            Mod::Alt => self.alt,
            Mod::Shift => self.shift,
        }
    }
    pub fn tap(&mut self, m: Mod) {
        let l = match m {
            Mod::Ctrl => &mut self.ctrl,
            Mod::Alt => &mut self.alt,
            Mod::Shift => &mut self.shift,
        };
        *l = l.tapped();
    }
    pub fn any(&self) -> bool {
        self.ctrl.on() || self.alt.on() || self.shift.on()
    }
    /// Alt+Shift locked: box (column) selection mode.
    pub fn box_mode(&self) -> bool {
        self.alt == Latch::Locked && self.shift == Latch::Locked
    }
    /// Shift locked on its own: selection mode.
    pub fn select_mode(&self) -> bool {
        self.shift == Latch::Locked && !self.box_mode()
    }
    fn spend(&mut self) {
        self.ctrl = self.ctrl.spent();
        self.alt = self.alt.spent();
        self.shift = self.shift.spent();
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// What a touch-toolbar button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Cmd(Command),
    Key(KeyCode),
    Mod(Mod),
    /// Toggle box-selection mode (Alt+Shift locked).
    BoxMode,
    /// Switch between the keys page and the actions page.
    Page,
}

#[derive(Clone, Copy, Debug)]
pub struct ToolKey {
    pub label: &'static str,
    /// Used when the button is too narrow for `label`.
    pub short: &'static str,
    pub tool: Tool,
}

const fn key(label: &'static str, short: &'static str, tool: Tool) -> ToolKey {
    ToolKey { label, short, tool }
}

/// The touch keyboard: a fixed 2×9 grid with a laptop-style arrow cluster
/// (↑ sits above ↓, with ← → beside it) and the modifiers on the left, like
/// Termux's extra keys. The second page holds editing actions.
pub const TOOLBAR_PAGES: [[[ToolKey; 9]; 2]; 2] = [
    [
        [
            key("Esc", "Esc", Tool::Key(KeyCode::Esc)),
            key("Ctrl", "Ctl", Tool::Mod(Mod::Ctrl)),
            key("Alt", "Alt", Tool::Mod(Mod::Alt)),
            key("Shift", "⇧", Tool::Mod(Mod::Shift)),
            key("Home", "⇱", Tool::Key(KeyCode::Home)),
            key("↑", "↑", Tool::Key(KeyCode::Up)),
            key("End", "⇲", Tool::Key(KeyCode::End)),
            key("PgUp", "⇞", Tool::Key(KeyCode::PageUp)),
            key("Fn", "Fn", Tool::Page),
        ],
        [
            key("Tab", "⇥", Tool::Key(KeyCode::Tab)),
            key("Box", "▦", Tool::BoxMode),
            key("Undo", "↶", Tool::Cmd(Command::Undo)),
            key("Redo", "↷", Tool::Cmd(Command::Redo)),
            key("←", "←", Tool::Key(KeyCode::Left)),
            key("↓", "↓", Tool::Key(KeyCode::Down)),
            key("→", "→", Tool::Key(KeyCode::Right)),
            key("PgDn", "⇟", Tool::Key(KeyCode::PageDown)),
            key("☰", "☰", Tool::Cmd(Command::ToggleSidebar)),
        ],
    ],
    [
        [
            key("Copy", "Cpy", Tool::Cmd(Command::Copy)),
            key("Cut", "Cut", Tool::Cmd(Command::Cut)),
            key("Paste", "Pst", Tool::Cmd(Command::Paste)),
            key("Save", "Sav", Tool::Cmd(Command::Save)),
            key("Find", "⌕", Tool::Cmd(Command::Find)),
            key("Repl", "⇄", Tool::Cmd(Command::Replace)),
            key("Line#", "#", Tool::Cmd(Command::GotoLine)),
            key("Cmds", "⋯", Tool::Cmd(Command::CommandPalette)),
            key("Keys", "⌨", Tool::Page),
        ],
        [
            key("All", "All", Tool::Cmd(Command::SelectAll)),
            key("Line", "Ln", Tool::Cmd(Command::SelectLine)),
            key("Word", "W", Tool::Cmd(Command::SelectNextOccurrence)),
            key("//", "//", Tool::Cmd(Command::ToggleComment)),
            key("Dup", "Dup", Tool::Cmd(Command::DuplicateLineDown)),
            key("Ln↑", "⇡", Tool::Cmd(Command::MoveLineUp)),
            key("Ln↓", "⇣", Tool::Cmd(Command::MoveLineDown)),
            key("Wrap", "↩", Tool::Cmd(Command::ToggleWordWrap)),
            key("AI", "✦", Tool::Cmd(Command::AgentToggle)),
        ],
    ],
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingClose {
    Tab(usize),
    Quit,
}

pub struct Dialog {
    pub title: String,
    pub message: String,
    pub action: PendingClose,
    pub selected: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub at: Instant,
}

#[derive(Default)]
pub struct FindState {
    pub visible: bool,
    pub show_replace: bool,
    pub query: String,
    pub replace: String,
    pub query_cursor: usize,
    pub replace_cursor: usize,
    pub on_replace: bool,
    /// The query was pre-filled from the editor and is shown selected.
    pub query_selected: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub error: bool,
    /// Char ranges of matches in the active document.
    pub matches: Vec<(usize, usize)>,
    pub current: Option<usize>,
    key: Option<(String, bool, bool, bool, u64, usize)>,
}

/// Which field of the find-in-files panel receives typing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchField {
    #[default]
    Query,
    Replace,
    /// Where to search (empty = the project root).
    Root,
    /// Directory filter: `src;!bin;!obj` (plain = scope, `!` = skip).
    Dirs,
    /// File filter: `*.cs;!*.csproj`.
    Filter,
}

/// Find/Replace in Files: the query, the file filter and the results.
#[derive(Default)]
pub struct SearchState {
    pub visible: bool,
    /// The replace inputs and buttons are shown.
    pub replace: bool,
    pub query: String,
    pub query_cursor: usize,
    pub replacement: String,
    pub replace_cursor: usize,
    /// Where to search. Empty means the project root; `~` and relative
    /// paths are accepted too (`~/code/app`, `../other`).
    pub root: String,
    pub root_cursor: usize,
    /// Directory filter: `src;!bin;!obj`. A plain entry scopes the search to
    /// that folder, `!` skips it (and everything under it).
    pub dirs: String,
    pub dirs_cursor: usize,
    /// File filter: `*.cs;!*.csproj`. A trailing `/` still means a folder.
    pub filter: String,
    pub filter_cursor: usize,
    /// `root` resolved against the project root, for display and searching.
    pub root_path: PathBuf,
    /// The root box names a folder that does not exist.
    pub missing_root: bool,
    pub field: SearchField,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub results: search::Results,
    pub error: bool,
    /// File headers the user collapsed.
    pub collapsed: HashSet<PathBuf>,
    /// Selected file index and match index.
    pub selected: Option<(usize, usize)>,
    pub scroll: usize,
    /// The query was pre-filled from the editor and is shown selected.
    pub query_selected: bool,
    key: Option<SearchKey>,
}

/// Everything a search depends on, so typing in the replace box does not
/// rebuild the results: query, file filter, directory filter, root, options,
/// the open editors' versions and the project root.
type SearchKey = (
    String,
    String,
    String,
    String,
    bool,
    bool,
    bool,
    u64,
    PathBuf,
);

/// Screen regions from the last frame, for mouse hit testing.
#[derive(Default, Clone)]
pub struct Layout {
    pub title_search: Rect,
    pub tabs: Vec<(Rect, Rect, usize)>,
    pub sidebar: Rect,
    pub sidebar_list: Rect,
    pub editor: Rect,
    pub gutter: Rect,
    pub text: Rect,
    pub minimap: Rect,
    pub scrollbar: Rect,
    pub palette_list: Rect,
    pub find_box: Rect,
    pub find_buttons: Vec<(Rect, u8)>,
    pub search_box: Rect,
    pub search_list: Rect,
    /// 0 = query, 1 = replace, 2 = root, 3 = directories, 4 = files.
    pub search_fields: Vec<(Rect, u8)>,
    /// 0..=2 options, 3 replace all, 4 replace file, 5 close.
    pub search_buttons: Vec<(Rect, u8)>,
    /// File headers (click to collapse/expand).
    pub search_files: Vec<(Rect, usize)>,
    /// Match rows (click to open).
    pub search_hits: Vec<(Rect, (usize, usize))>,
    /// Per-match replace buttons.
    pub search_replace_hits: Vec<(Rect, (usize, usize))>,
    pub dialog_buttons: Vec<(Rect, usize)>,
    pub click_items: Vec<(Rect, Command)>,
    pub toolbar: Vec<(Rect, Tool)>,
    /// Explorer shown as a slide-over drawer (compact layout).
    pub drawer: Rect,
    pub agent_panel: Rect,
    pub agent_input: Rect,
    pub agent_targets: Vec<(Rect, AgentTarget)>,
}

/// Tappable things in the agent panel.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentTarget {
    OpenFile(PathBuf, Option<u32>),
    Permission(usize, String),
    Retry,
    NewChat,
    Close,
    Model,
    PickAgent,
    Settings,
    Stop,
    Send,
    /// A one-tap starter prompt.
    Suggest(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    Text,
    Box,
    Scrollbar,
    Minimap,
    SidebarResize,
}

pub struct Settings {
    pub minimap: bool,
    pub whitespace: bool,
    pub indent_guides: bool,
    pub line_numbers: bool,
    pub nerd_font: bool,
    /// Soft wrap; `None` = automatic (on in the compact/phone layout).
    pub word_wrap: Option<bool>,
    /// Touch toolbar; `None` = automatic (on in the compact/phone layout).
    pub toolbar: Option<bool>,
    /// Always use the compact layout (`--mobile`).
    pub force_compact: bool,
}

pub struct App {
    pub editors: Vec<Editor>,
    pub active: usize,
    pub explorer: Explorer,
    pub show_sidebar: bool,
    pub sidebar_width: u16,
    pub focus: Focus,
    pub theme: ThemeEntry,
    pub palette: Option<Palette>,
    pub dialog: Option<Dialog>,
    pub find: FindState,
    pub search: SearchState,
    pub toasts: Vec<Toast>,
    pub settings: Settings,
    pub preferences: Preferences,
    pub skills: Vec<Skill>,
    pub management: Management,
    pub automation: Automation,
    pub git_branch: Option<String>,
    pub layout: Layout,
    pub quit: bool,
    /// Phone-sized screen: single-column layout (set by the renderer each frame).
    pub compact: bool,
    /// Latched modifiers from the touch toolbar.
    pub mods: StickyMods,
    pub agent: Agent,
    /// External plugin processes (see `plugin.rs`).
    pub plugins: PluginHost,
    pub show_agent: bool,
    pub toolbar_page: usize,
    drawer_init: bool,
    clipboard: Clipboard,
    pub chord: bool,
    drag: Option<Drag>,
    last_click: Option<(Instant, u16, u16, u8)>,
    project_files: Option<Vec<String>>,
}

impl App {
    pub fn new(root: PathBuf, files: Vec<PathBuf>, theme_name: &str, nerd_font: bool) -> Self {
        let root = search::absolute_path(&root);
        let (preferences, config_error) = match Preferences::load(&root) {
            Ok(p) => (p, None),
            Err(e) => (Preferences::default(), Some(format!("{e:#}"))),
        };
        let saved = &preferences.editor;
        let theme_name = if theme_name == "Visual Studio Dark" && !saved.theme.is_empty() {
            &saved.theme
        } else {
            theme_name
        };
        let theme =
            theme::load(theme_name).unwrap_or_else(|| theme::load("Visual Studio Dark").unwrap());
        let mut app = Self {
            editors: Vec::new(),
            active: 0,
            explorer: Explorer::new(root.clone()),
            show_sidebar: true,
            sidebar_width: 30,
            focus: Focus::Editor,
            theme,
            palette: None,
            dialog: None,
            find: FindState {
                case_sensitive: false,
                ..Default::default()
            },
            search: SearchState::default(),
            toasts: Vec::new(),
            settings: Settings {
                minimap: saved.minimap,
                whitespace: saved.whitespace,
                indent_guides: saved.indent_guides,
                line_numbers: saved.line_numbers,
                nerd_font,
                word_wrap: saved.word_wrap,
                toolbar: saved.toolbar,
                force_compact: false,
            },
            skills: crate::skills::discover(&root, &preferences.disabled_skills),
            preferences,
            management: Management::default(),
            automation: Automation::default(),
            compact: false,
            mods: StickyMods::default(),
            agent: Agent::new(root.clone()),
            plugins: PluginHost::new(root.clone()),
            show_agent: false,
            toolbar_page: 0,
            drawer_init: false,
            git_branch: git_branch(&root),
            layout: Layout::default(),
            quit: false,
            clipboard: Clipboard::default(),
            chord: false,
            drag: None,
            last_click: None,
            project_files: None,
        };
        if let Err(e) = app.agent.configure(
            app.preferences.agent.clone(),
            crate::skills::context(&app.skills, &app.preferences.agent.skills),
        ) {
            app.agent.state = AgentState::Failed(e.clone());
            app.toast(e, ToastKind::Error);
        }
        if let Some(error) = config_error {
            app.management.load_error = Some(error.clone());
            app.toast(format!("Settings not loaded: {error}"), ToastKind::Error);
        } else if crate::config::user_settings_path().exists()
            && crate::config::settings_path(&root).exists()
            && search::absolute_path(&crate::config::user_settings_path())
                != search::absolute_path(&crate::config::settings_path(&root))
        {
            app.toast(
                "User settings active; legacy workspace restrictions and settings are not applied",
                ToastKind::Info,
            );
        }
        for f in files {
            app.open_file(&f);
        }
        if app.editors.is_empty() {
            app.focus = Focus::Explorer;
        }
        // Plugins that asked for `startup` can begin before the first frame.
        app.plugins.warm_up();
        app
    }

    pub fn editor(&self) -> Option<&Editor> {
        self.editors.get(self.active)
    }

    pub fn editor_mut(&mut self) -> Option<&mut Editor> {
        self.editors.get_mut(self.active)
    }

    pub fn toast(&mut self, text: impl Into<String>, kind: ToastKind) {
        self.toasts.push(Toast {
            text: text.into(),
            kind,
            at: Instant::now(),
        });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    pub(crate) fn invalidate_project_files(&mut self) {
        self.project_files = None;
    }

    pub fn word_wrap(&self) -> bool {
        self.settings.word_wrap.unwrap_or(self.compact)
    }

    pub fn toolbar_visible(&self) -> bool {
        self.settings.toolbar.unwrap_or(self.compact)
    }

    /// Called by the renderer with the screen width; switches the layout mode.
    pub fn set_compact(&mut self, compact: bool) {
        let compact = compact || self.settings.force_compact;
        if compact && !self.drawer_init {
            // On a phone the explorer is a drawer: start closed when a file is open.
            self.show_sidebar = self.editors.is_empty();
            self.drawer_init = true;
        }
        self.compact = compact;
    }

    /// Background work (highlighting) wants another frame.
    pub fn busy(&self) -> bool {
        self.editor().is_some_and(|e| e.hl_pending)
    }

    /// Expire old notifications; returns true if anything changed.
    pub fn tick(&mut self) -> bool {
        let old_search = self.search.key.clone();
        self.update_search();
        let before = self.toasts.len();
        self.toasts
            .retain(|t| t.at.elapsed() < Duration::from_millis(3500));
        before != self.toasts.len() || old_search != self.search.key
    }

    pub fn open_file(&mut self, path: &Path) {
        let path = search::absolute_path(path);
        if crate::credentials::is_private_path(&path) {
            self.toast(
                "Credential files are private; manage the router key through masked AI settings",
                ToastKind::Error,
            );
            return;
        }
        if let Some(i) = self
            .editors
            .iter()
            .position(|e| e.doc.path.as_deref() == Some(&path))
        {
            // An explicit reopen refreshes a cached binary preview. Ordinary
            // text buffers retain their unsaved changes and undo history.
            if self.editors[i].doc.is_read_only() {
                match Editor::open(path.clone()) {
                    Ok(editor) => self.editors[i] = editor,
                    Err(error) => {
                        self.toast(
                            format!("Cannot refresh preview: {error:#}"),
                            ToastKind::Error,
                        );
                        return;
                    }
                }
            }
            self.active = i;
        } else if path.is_dir() {
            return;
        } else {
            match Editor::open(path.clone()) {
                Ok(e) => {
                    self.editors.push(e);
                    self.active = self.editors.len() - 1;
                }
                Err(err) => {
                    self.toast(
                        format!("Cannot open {}: {err:#}", path.display()),
                        ToastKind::Error,
                    );
                    return;
                }
            }
        }
        self.focus = Focus::Editor;
        self.explorer.reveal(&path);
        self.plugins
            .broadcast("open", json!({ "path": path.to_string_lossy() }));
    }

    pub fn set_theme(&mut self, name: &str) {
        if let Some(t) = theme::load(name) {
            self.theme = t;
            for e in &mut self.editors {
                e.hl.invalidate_all();
            }
        }
    }

    // ================================================================ input

    pub fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
            Event::Mouse(m) => self.on_mouse(m),
            Event::Paste(text) => self.on_paste(&text),
            _ => {}
        }
    }

    fn on_paste(&mut self, text: &str) {
        if self.focus == Focus::Agent && self.palette.is_none() {
            let a = &mut self.agent;
            a.input.insert_str(byte_at(&a.input, a.cursor), text);
            a.cursor += text.chars().count();
            return;
        }
        if let Some(p) = &mut self.palette {
            let t = text.replace(['\n', '\r'], "");
            if std::mem::take(&mut p.input_selected) {
                p.input.clear();
                p.cursor = 0;
            }
            p.input.insert_str(byte_at(&p.input, p.cursor), &t);
            p.cursor += t.chars().count();
            self.refilter();
        } else if self.focus == Focus::Find {
            if self.find.query_selected && !self.find.on_replace {
                self.find.query.clear();
                self.find.query_cursor = 0;
                self.find.query_selected = false;
            }
            let t = text.replace(['\n', '\r'], "");
            let (field, cur) = self.find_field();
            field.insert_str(byte_at(field, *cur), &t);
            *cur += t.chars().count();
        } else if self.focus == Focus::Search {
            if self.search.query_selected && self.search.field == SearchField::Query {
                self.search.query.clear();
                self.search.query_cursor = 0;
                self.search.query_selected = false;
            }
            let t = text.replace(['\n', '\r'], "");
            let (field, cur) = self.search_field_mut();
            field.insert_str(byte_at(field, *cur), &t);
            *cur += t.chars().count();
            self.update_search();
        } else if let Some(e) = self.editor_mut() {
            e.insert_text(text);
            e.scroll_into_view();
        }
    }

    /// Fold latched toolbar modifiers into a key event, then spend the
    /// one-shot ones. Locked Shift is "selection mode" and locked Alt+Shift
    /// is "box mode": they steer movement keys but let typing through, so
    /// you can type into a box selection or replace a selection normally.
    fn apply_sticky(&self, mut k: KeyEvent) -> KeyEvent {
        let m = self.mods;
        let typing = matches!(
            k.code,
            KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete | KeyCode::Enter
        );
        let applies = |latch: Latch, is_shift_or_alt: bool, is_shift: bool| match latch {
            Latch::Off => false,
            Latch::Once => true,
            Latch::Locked => {
                if m.box_mode() && is_shift_or_alt || is_shift {
                    !typing
                } else {
                    true
                }
            }
        };
        if applies(m.ctrl, false, false) {
            k.modifiers |= KeyModifiers::CONTROL;
        }
        if applies(m.alt, true, false) {
            k.modifiers |= KeyModifiers::ALT;
        }
        if applies(m.shift, true, true) {
            k.modifiers |= KeyModifiers::SHIFT;
            if let KeyCode::Char(c) = k.code
                && !k
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                k.code = KeyCode::Char(c.to_uppercase().next().unwrap_or(c));
            }
        }
        k
    }

    /// Synthesize a key press (touch toolbar).
    pub fn press(&mut self, code: KeyCode) {
        self.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn on_key(&mut self, k: KeyEvent) {
        let typing = matches!(
            k.code,
            KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete | KeyCode::Enter
        );
        let esc = k.code == KeyCode::Esc;
        self.on_key_inner(k);
        self.mods.spend();
        if esc {
            // Esc always releases latched/locked modifiers.
            self.mods.clear();
        } else if typing && self.mods.select_mode() && self.focus == Focus::Editor {
            // Typing over a selection ends selection mode.
            self.mods.shift = Latch::Off;
        }
    }

    fn on_key_inner(&mut self, mut k: KeyEvent) {
        // Some phone keyboards send LF for Return, which arrives as Ctrl+J.
        if k.code == KeyCode::Char('j') && k.modifiers == KeyModifiers::CONTROL {
            k = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        }
        if self.mods.any() {
            k = self.apply_sticky(k);
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        if k.code == KeyCode::Tab && shift {
            k.code = KeyCode::BackTab;
        }
        // Normalize Ctrl+Shift+<letter>, which may arrive upper- or lower-case.
        let code = match k.code {
            KeyCode::Char(c) if ctrl || alt => KeyCode::Char(c.to_ascii_lowercase()),
            c => c,
        };

        if self.dialog.is_some() {
            self.dialog_key(code);
            return;
        }
        if self.palette.is_some() {
            self.palette_key(code, ctrl);
            return;
        }

        if self.chord {
            self.chord = false;
            let cmd = match code {
                KeyCode::Char('t') => Some(Command::SelectTheme),
                KeyCode::Char('a') => Some(Command::AgentToggle),
                KeyCode::Char('m') => Some(if ctrl {
                    Command::SelectLanguage
                } else {
                    Command::ToggleMinimap
                }),
                KeyCode::Char('w') => Some(Command::ToggleWhitespace),
                KeyCode::Char('i') => Some(Command::ToggleIndentGuides),
                KeyCode::Char('l') => Some(Command::ToggleLineNumbers),
                KeyCode::Char('s') => Some(if ctrl {
                    Command::ShowKeybindings
                } else {
                    Command::SaveAll
                }),
                _ => None,
            };
            if let Some(c) = cmd {
                self.run(c);
            }
            return;
        }

        // Global bindings.
        let global = match (code, ctrl, shift, alt) {
            (KeyCode::Char('p'), true, true, _) | (KeyCode::F(1), ..) => {
                Some(Command::CommandPalette)
            }
            // Ctrl+Alt+P is the plugin manager, so it has to be matched
            // before plain Ctrl+P opens quick open.
            (KeyCode::Char('p'), true, _, true) => Some(Command::Plugins),
            (KeyCode::Char('p'), true, false, _) | (KeyCode::Char('o'), true, ..) => {
                Some(Command::QuickOpen)
            }
            (KeyCode::Char(','), true, ..) => Some(Command::Settings),
            (KeyCode::Char('q'), true, ..) => Some(Command::Quit),
            (KeyCode::Char('n'), true, ..) => Some(Command::NewFile),
            (KeyCode::Char('s'), true, _, true) => Some(Command::SaveAs),
            (KeyCode::Char('s'), true, true, _) => Some(Command::SaveAs),
            (KeyCode::Char('s'), true, ..) => Some(Command::Save),
            (KeyCode::Char('w'), true, ..) => Some(Command::CloseTab),
            (KeyCode::Char('b'), true, ..) => Some(Command::ToggleSidebar),
            (KeyCode::Char('e'), true, true, _) | (KeyCode::Char('e'), true, ..) => {
                Some(Command::FocusExplorer)
            }
            (KeyCode::PageDown, true, ..) | (KeyCode::Tab, true, false, _) => {
                Some(Command::NextTab)
            }
            (KeyCode::PageUp, true, ..) | (KeyCode::BackTab, true, ..) => Some(Command::PrevTab),
            (KeyCode::Char('f'), true, true, _) => Some(Command::FindInFiles),
            (KeyCode::Char('h'), true, true, _) => Some(Command::ReplaceInFiles),
            (KeyCode::Char('f'), true, ..) => Some(Command::Find),
            (KeyCode::Char('h'), true, ..) => Some(Command::Replace),
            (KeyCode::Char('g'), true, ..) => Some(Command::GotoLine),
            (KeyCode::F(3), _, true, _) => Some(Command::FindPrev),
            (KeyCode::F(3), ..) => Some(Command::FindNext),
            (KeyCode::F(8), ..) => Some(Command::NextTheme),
            _ => None,
        };
        if let Some(c) = global {
            self.run(c);
            return;
        }
        if let Some((pi, ci)) = self.plugins.key_action(code, k.modifiers) {
            self.run_plugin(pi, ci);
            return;
        }
        if ctrl && code == KeyCode::Char('k') {
            self.chord = true;
            return;
        }
        if alt && !ctrl && code == KeyCode::Char('z') {
            self.run(Command::ToggleWordWrap);
            return;
        }
        if alt
            && !ctrl
            && let KeyCode::Char(c @ '1'..='9') = code
        {
            let i = c as usize - '1' as usize;
            if i < self.editors.len() {
                self.active = i;
                self.focus = Focus::Editor;
            }
            return;
        }

        match self.focus {
            Focus::Find => self.find_key(code, ctrl, shift, alt),
            Focus::Search => self.search_key(code, ctrl, shift, alt),
            Focus::Agent => self.agent_key(code, ctrl, shift, alt),
            Focus::Explorer => {
                if shift && code == KeyCode::Enter {
                    self.run(Command::OpenArchiveFile);
                } else {
                    self.explorer_key(code);
                }
            }
            Focus::Editor => self.editor_key(code, ctrl, shift, alt),
        }
    }

    fn editor_key(&mut self, code: KeyCode, ctrl: bool, shift: bool, alt: bool) {
        if !ctrl
            && !alt
            && let Some(crate::document::Preview::Image(image)) =
                self.editor_mut().and_then(|e| e.doc.preview.as_mut())
        {
            image.key(code);
            return;
        }
        let cmd = match (code, ctrl, shift, alt) {
            (KeyCode::Char('z'), true, true, _) => Some(Command::Redo),
            (KeyCode::Char('z'), true, ..) => Some(Command::Undo),
            (KeyCode::Char('y'), true, ..) => Some(Command::Redo),
            (KeyCode::Char('x'), true, ..) => Some(Command::Cut),
            (KeyCode::Char('c'), true, ..) | (KeyCode::Insert, true, ..) => Some(Command::Copy),
            (KeyCode::Char('v'), true, ..) | (KeyCode::Insert, _, true, _) => Some(Command::Paste),
            (KeyCode::Char('a'), true, ..) => Some(Command::SelectAll),
            (KeyCode::Char('l'), true, ..) => Some(Command::SelectLine),
            (KeyCode::Char('d'), true, true, _) => Some(Command::DuplicateLineDown),
            (KeyCode::Char('d'), true, ..) => Some(Command::SelectNextOccurrence),
            (KeyCode::Char('/'), true, ..) | (KeyCode::Char('7'), true, ..) => {
                Some(Command::ToggleComment)
            }
            // Legacy terminals encode Ctrl+\ and Ctrl+] as Ctrl+4 and Ctrl+5.
            (KeyCode::Char('\\' | '|' | '4'), true, ..) => Some(Command::GotoBracket),
            (KeyCode::Char(']' | '5'), true, ..) => Some(Command::Indent),
            (KeyCode::Char('['), true, ..) => Some(Command::Outdent),
            (KeyCode::Char('k'), true, true, _) => Some(Command::DeleteLine),
            (KeyCode::Up, false, false, true) => Some(Command::MoveLineUp),
            (KeyCode::Down, false, false, true) => Some(Command::MoveLineDown),
            (KeyCode::Enter, true, true, _) => Some(Command::InsertLineAbove),
            (KeyCode::Enter, true, ..) => Some(Command::InsertLineBelow),
            _ => None,
        };
        if let Some(c) = cmd {
            self.run(c);
            return;
        }
        let find_visible = self.find.visible;
        let Some(e) = self.editors.get_mut(self.active) else {
            return;
        };
        // Visual Studio box selection: Alt+Shift+arrows.
        if alt && shift && !ctrl {
            let (dl, dc) = match code {
                KeyCode::Up => (-1, 0),
                KeyCode::Down => (1, 0),
                KeyCode::Left => (0, -1),
                KeyCode::Right => (0, 1),
                KeyCode::PageUp => (-(e.view_height.max(2) as isize - 1), 0),
                KeyCode::PageDown => (e.view_height.max(2) as isize - 1, 0),
                _ => (0, 0),
            };
            if (dl, dc) != (0, 0) {
                e.box_extend(dl, dc);
                e.scroll_into_view();
                return;
            }
        }
        if e.block.is_some() {
            let handled = match code {
                KeyCode::Char(c) if !ctrl && !alt => {
                    e.box_type(&c.to_string());
                    true
                }
                KeyCode::Tab => {
                    let unit = e.doc.indent.unit();
                    e.box_type(&unit);
                    true
                }
                KeyCode::Backspace => {
                    e.box_backspace();
                    true
                }
                KeyCode::Delete => {
                    e.box_delete();
                    true
                }
                KeyCode::Esc => {
                    e.box_clear();
                    true
                }
                _ => {
                    // Any other key leaves box mode and acts normally.
                    e.box_clear();
                    false
                }
            };
            if handled {
                e.scroll_into_view();
                return;
            }
        }
        match code {
            KeyCode::Esc => {
                if self.search.visible {
                    self.search.visible = false;
                } else if find_visible {
                    self.find.visible = false;
                } else {
                    e.sel = Selection::caret(e.sel.head);
                }
            }
            KeyCode::Left if ctrl => e.move_word(false, shift),
            KeyCode::Right if ctrl => e.move_word(true, shift),
            KeyCode::Left => e.move_horizontal(-1, shift),
            KeyCode::Right => e.move_horizontal(1, shift),
            KeyCode::Up if ctrl => {
                e.scroll_by(-1);
                return;
            }
            KeyCode::Down if ctrl => {
                e.scroll_by(1);
                return;
            }
            KeyCode::Up => e.move_vertical(-1, shift),
            KeyCode::Down => e.move_vertical(1, shift),
            KeyCode::Home if ctrl => e.doc_start(shift),
            KeyCode::End if ctrl => e.doc_end(shift),
            KeyCode::Home => e.home(shift),
            KeyCode::End => e.end(shift),
            KeyCode::PageUp => e.page(false, shift),
            KeyCode::PageDown => e.page(true, shift),
            KeyCode::Backspace if ctrl || alt => e.delete_word(false),
            KeyCode::Delete if ctrl => e.delete_word(true),
            KeyCode::Backspace => e.backspace(),
            KeyCode::Delete => e.delete_forward(),
            KeyCode::Enter => e.newline(),
            KeyCode::Tab => e.tab_key(),
            KeyCode::BackTab => e.indent_lines(false),
            KeyCode::Char(c) if !ctrl && !alt => e.type_char(c),
            _ => return,
        }
        e.scroll_into_view();
    }

    fn explorer_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up => self.explorer.move_selection(-1),
            KeyCode::Down => self.explorer.move_selection(1),
            KeyCode::PageUp => self
                .explorer
                .move_selection(-(self.layout.sidebar_list.height as isize)),
            KeyCode::PageDown => self
                .explorer
                .move_selection(self.layout.sidebar_list.height as isize),
            KeyCode::Home => self.explorer.selected = 0,
            KeyCode::End => self.explorer.selected = self.explorer.entries.len().saturating_sub(1),
            KeyCode::Right => self.explorer.expand(),
            KeyCode::Left => self.explorer.collapse(),
            KeyCode::Enter | KeyCode::Char(' ') => self.activate_explorer_entry(),
            KeyCode::Esc | KeyCode::Tab if !self.editors.is_empty() => {
                self.focus = Focus::Editor;
            }
            _ => {}
        }
        let errors = std::mem::take(&mut self.explorer.errors);
        for error in errors {
            self.toast(error, ToastKind::Error);
        }
    }

    fn activate_explorer_entry(&mut self) {
        if let Some(e) = self.explorer.selected_entry().cloned() {
            if e.is_dir {
                self.explorer.toggle(self.explorer.selected);
                let errors = std::mem::take(&mut self.explorer.errors);
                for error in errors {
                    self.toast(error, ToastKind::Error);
                }
            } else {
                self.open_file(&e.path);
                if self.compact {
                    // The drawer gets out of the way once a file is picked.
                    self.show_sidebar = false;
                }
            }
        }
    }

    // ================================================================ commands

    pub fn run(&mut self, cmd: Command) {
        use Command::*;
        match cmd {
            ImageZoomIn | ImageZoomOut | ImageFit => {
                if let Some(crate::document::Preview::Image(image)) =
                    self.editor_mut().and_then(|e| e.doc.preview.as_mut())
                {
                    match cmd {
                        ImageZoomIn => image.zoom_by(1),
                        ImageZoomOut => image.zoom_by(-1),
                        _ => image.fit(),
                    }
                } else {
                    self.toast("Open an image to use preview controls", ToastKind::Info);
                }
            }
            OpenArchiveFile => {
                if let Some(e) = self.explorer.selected_entry().cloned()
                    && e.is_archive
                {
                    self.open_file(&e.path);
                    if self.compact {
                        self.show_sidebar = false;
                    }
                } else {
                    self.toast("Select a ZIP in Explorer first", ToastKind::Info);
                }
            }
            QuickOpen => self.open_palette(PaletteMode::Files),
            CommandPalette | ShowKeybindings => self.open_palette(PaletteMode::Commands),
            GotoLine => {
                if self.editor().is_some() {
                    self.open_palette(PaletteMode::GotoLine);
                }
            }
            SelectTheme => self.open_palette(PaletteMode::Themes),
            SelectLanguage => {
                if self.editor().is_some() {
                    self.open_palette(PaletteMode::Languages);
                }
            }
            SaveAs => {
                if self.editor().is_some() {
                    self.open_palette(PaletteMode::SaveAs);
                }
            }
            NextTheme => {
                let names = theme::theme_names();
                let i = names
                    .iter()
                    .position(|n| *n == self.theme.name)
                    .unwrap_or(0);
                let next = names[(i + 1) % names.len()];
                self.set_theme(next);
                self.toast(format!("Theme: {next}"), ToastKind::Info);
            }
            NewFile => {
                self.editors.push(Editor::new(Document::empty()));
                self.active = self.editors.len() - 1;
                self.focus = Focus::Editor;
            }
            Save => self.save_active(),
            SaveAll => {
                let mut n = 0;
                let mut saved = Vec::new();
                for e in &mut self.editors {
                    if e.doc.is_dirty() && e.doc.path.is_some() && e.doc.save().is_ok() {
                        n += 1;
                        if let Some(p) = &e.doc.path {
                            saved.push(p.clone());
                        }
                    }
                }
                self.toast(format!("Saved {n} file(s)"), ToastKind::Success);
                for path in saved {
                    self.plugins
                        .broadcast("save", json!({ "path": path.to_string_lossy() }));
                }
            }
            CloseTab => self.request_close(PendingClose::Tab(self.active)),
            Quit => self.request_close(PendingClose::Quit),
            NextTab | PrevTab => {
                if !self.editors.is_empty() {
                    let n = self.editors.len();
                    self.active = if cmd == NextTab {
                        (self.active + 1) % n
                    } else {
                        (self.active + n - 1) % n
                    };
                    self.focus = Focus::Editor;
                }
            }
            ToggleSidebar => {
                self.show_sidebar = !self.show_sidebar;
                if !self.show_sidebar && self.focus == Focus::Explorer {
                    self.focus = Focus::Editor;
                }
            }
            FocusExplorer => {
                if self.focus == Focus::Explorer && !self.editors.is_empty() {
                    self.focus = Focus::Editor;
                } else {
                    self.show_sidebar = true;
                    self.focus = Focus::Explorer;
                }
            }
            ToggleMinimap => self.settings.minimap = !self.settings.minimap,
            AgentToggle => {
                if self.show_agent && self.focus == Focus::Agent {
                    self.show_agent = false;
                    self.focus = if self.editors.is_empty() {
                        Focus::Explorer
                    } else {
                        Focus::Editor
                    };
                } else {
                    self.show_agent = true;
                    self.focus = Focus::Agent;
                    if self.compact {
                        self.show_sidebar = false;
                    }
                }
            }
            AgentNewChat => {
                self.cancel_automation();
                self.agent.cancel();
                self.agent.new_chat();
                self.show_agent = true;
                self.focus = Focus::Agent;
            }
            AgentStop => {
                self.agent.cancel();
                self.cancel_automation();
                self.agent.stop();
                self.agent.entries.push(agent::Entry::Notice{text:"Stopped this task tree. Partial edits were kept for review; no side effects were automatically retried. Provider session reset: restate essential context before continuing.".into(),error:false});
            }
            AgentModel => {
                if self.agent.models.is_empty() {
                    self.toast(
                        "Send a first message to load the model list",
                        ToastKind::Info,
                    );
                } else {
                    self.open_palette(PaletteMode::AgentModels);
                }
            }
            AgentPick => {
                self.agent.available = agent::detect_agents();
                if self.agent.available.is_empty() {
                    self.show_agent = true;
                    self.agent.state = AgentState::NoAgent;
                } else {
                    self.open_palette(PaletteMode::AgentPicker);
                }
            }
            ToggleWordWrap => {
                let on = !self.word_wrap();
                self.settings.word_wrap = Some(on);
                let state = if on { "on" } else { "off" };
                self.toast(format!("Word wrap {state}"), ToastKind::Info);
            }
            ToggleToolbar => self.settings.toolbar = Some(!self.toolbar_visible()),
            ToggleSelectMode => {
                self.mods.shift = if self.mods.select_mode() {
                    Latch::Off
                } else {
                    Latch::Locked
                };
                if !self.editors.is_empty() {
                    self.focus = Focus::Editor;
                }
            }
            ToggleWhitespace => self.settings.whitespace = !self.settings.whitespace,
            ToggleIndentGuides => self.settings.indent_guides = !self.settings.indent_guides,
            ToggleLineNumbers => self.settings.line_numbers = !self.settings.line_numbers,
            RefreshExplorer => self.explorer.refresh(),
            RevealInExplorer => {
                if let Some(p) = self.editor().and_then(|e| e.doc.path.clone()) {
                    self.show_sidebar = true;
                    self.explorer.reveal(&p);
                    self.focus = Focus::Explorer;
                }
            }
            FindInFiles => self.open_search(false),
            ReplaceInFiles => self.open_search(true),
            Plugins => self.open_palette(PaletteMode::Plugins),
            Settings => self.open_palette(PaletteMode::Settings),
            AgentSettings => self.open_palette(PaletteMode::AgentSettings),
            SubAgents => self.open_palette(PaletteMode::SubAgents),
            Skills => self.open_palette(PaletteMode::Skills),
            Find | Replace => {
                self.search.visible = false;
                self.find.visible = true;
                self.find.show_replace = cmd == Replace;
                self.find.on_replace = false;
                if let Some(e) = self.editor() {
                    let sel = e.selected_text();
                    if !sel.is_empty() && !sel.contains('\n') {
                        self.find.query = sel;
                    } else if sel.is_empty()
                        && let Some((s, en)) = e.word_at(e.sel.head)
                    {
                        self.find.query = e.doc.rope.slice(s..en).to_string();
                    }
                }
                self.find.query_cursor = self.find.query.chars().count();
                self.find.query_selected = !self.find.query.is_empty();
                self.focus = Focus::Find;
                self.update_find();
                self.find_jump(true, true);
            }
            FindNext | FindPrev => {
                if self.find.query.is_empty() {
                    self.run(Find);
                } else {
                    self.update_find();
                    self.find_jump(cmd == FindNext, false);
                }
            }
            ConvertIndentToSpaces | ConvertIndentToTabs => {
                self.convert_indent(cmd == ConvertIndentToTabs)
            }
            ToggleLineEnding => {
                if let Some(e) = self.editor_mut() {
                    e.doc.line_ending = match e.doc.line_ending {
                        LineEnding::Lf => LineEnding::CrLf,
                        LineEnding::CrLf => LineEnding::Lf,
                    };
                    e.doc.mark_modified();
                    let le = e.doc.line_ending;
                    let label = le.label();
                    self.toast(format!("Line endings: {label}"), ToastKind::Info);
                }
            }
            GotoBracket => {
                let bg = self.theme.palette.bg;
                if let Some(e) = self.editors.get_mut(self.active) {
                    let (line, _) = e.cursor();
                    e.hl.ensure(&e.doc.rope, line + 5000, &self.theme.syntax, bg);
                    e.jump_to_bracket();
                    e.scroll_into_view();
                }
            }
            Undo | Redo | Cut | Copy | Paste | SelectAll | SelectLine | SelectNextOccurrence
            | ToggleComment | Indent | Outdent | MoveLineUp | MoveLineDown | DuplicateLineDown
            | DuplicateLineUp | DeleteLine | InsertLineBelow | InsertLineAbove => {
                self.edit_command(cmd)
            }
        }
        if matches!(
            cmd,
            ToggleMinimap
                | ToggleWhitespace
                | ToggleIndentGuides
                | ToggleLineNumbers
                | ToggleWordWrap
                | ToggleToolbar
                | NextTheme
        ) {
            self.persist_editor_preferences();
        }
    }

    fn edit_command(&mut self, cmd: Command) {
        use Command::*;
        if self.editors.is_empty() {
            return;
        }
        // Clipboard on a box selection works per line.
        if self.editors[self.active].block.is_some() {
            match cmd {
                Copy | Cut => {
                    let e = &mut self.editors[self.active];
                    let text = e.box_text();
                    if cmd == Cut && !e.block.is_some_and(|b| b.is_carets()) {
                        e.box_delete();
                    }
                    self.clipboard.set(text, false);
                    return;
                }
                Paste => {
                    let text = self.clipboard.get();
                    let e = &mut self.editors[self.active];
                    e.box_paste(&text);
                    e.scroll_into_view();
                    return;
                }
                Undo | Redo => {}
                _ => self.editors[self.active].box_clear(),
            }
        }
        if cmd == Paste {
            let whole = self.clipboard.whole_line;
            let text = self.clipboard.get();
            let whole = whole && self.clipboard.whole_line;
            if let Some(e) = self.editor_mut() {
                if whole && e.sel.is_empty() {
                    e.insert_lines_above(&text);
                } else {
                    e.insert_text(&text);
                }
                e.scroll_into_view();
            }
            return;
        }
        let e = &mut self.editors[self.active];
        let mut clip = None;
        match cmd {
            Undo => e.undo(),
            Redo => e.redo(),
            Cut => clip = Some(e.cut()),
            Copy => clip = Some(e.copy()),
            SelectAll => e.select_all(),
            SelectLine => e.select_line(),
            SelectNextOccurrence => {
                e.select_next_occurrence();
            }
            ToggleComment => e.toggle_comment(),
            Indent => e.indent_lines(true),
            Outdent => e.indent_lines(false),
            MoveLineUp => e.move_lines(false),
            MoveLineDown => e.move_lines(true),
            DuplicateLineDown => e.duplicate_lines(true),
            DuplicateLineUp => e.duplicate_lines(false),
            DeleteLine => e.delete_lines(),
            InsertLineBelow => e.insert_line(true),
            InsertLineAbove => e.insert_line(false),
            _ => {}
        }
        e.scroll_into_view();
        if let Some((text, whole)) = clip {
            if self.mods.select_mode() {
                self.mods.shift = Latch::Off;
            }
            self.clipboard.set(text, whole);
        }
    }

    fn convert_indent(&mut self, to_tabs: bool) {
        let Some(e) = self.editor_mut() else { return };
        let width = match e.doc.indent {
            Indent::Spaces(n) => n,
            Indent::Tabs => e.doc.tab_width,
        };
        let text = e.doc.rope.to_string();
        let converted: Vec<String> = text
            .split('\n')
            .map(|line| {
                let ws: String = line
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                let cols: usize = ws.chars().map(|c| if c == '\t' { width } else { 1 }).sum();
                let new_ws = if to_tabs {
                    format!("{}{}", "\t".repeat(cols / width), " ".repeat(cols % width))
                } else {
                    " ".repeat(cols)
                };
                format!("{new_ws}{}", &line[ws.len()..])
            })
            .collect();
        let new = converted.join("\n");
        let head = e.sel.head;
        e.replace_range(0, e.doc.rope.len_chars(), &new);
        e.sel = Selection::caret(head.min(e.doc.rope.len_chars()));
        e.doc.indent = if to_tabs {
            Indent::Tabs
        } else {
            Indent::Spaces(width)
        };
    }

    fn save_active(&mut self) {
        let Some(e) = self.editor_mut() else { return };
        if e.doc.path.is_none() {
            self.open_palette(PaletteMode::SaveAs);
            return;
        }
        match e.doc.save() {
            Ok(()) => {
                let name = e.doc.name();
                let lines = e.doc.len_lines();
                let saved = e.doc.path.clone();
                self.toast(
                    format!("Saved {name}  ·  {lines} lines"),
                    ToastKind::Success,
                );
                self.project_files = None;
                if let Some(path) = saved {
                    self.plugins
                        .broadcast("save", json!({ "path": path.to_string_lossy() }));
                }
            }
            Err(err) => self.toast(format!("Save failed: {err:#}"), ToastKind::Error),
        }
    }

    fn request_close(&mut self, what: PendingClose) {
        let dirty: Vec<usize> = match what {
            PendingClose::Tab(i) => self
                .editors
                .get(i)
                .filter(|e| e.doc.is_dirty())
                .map(|_| vec![i])
                .unwrap_or_default(),
            PendingClose::Quit => (0..self.editors.len())
                .filter(|i| self.editors[*i].doc.is_dirty())
                .collect(),
        };
        if dirty.is_empty() {
            self.do_close(what);
            return;
        }
        let message = if dirty.len() == 1 {
            format!(
                "Do you want to save the changes you made to {}?",
                self.editors[dirty[0]].doc.name()
            )
        } else {
            format!("Do you want to save the changes to {} files?", dirty.len())
        };
        self.dialog = Some(Dialog {
            title: "Unsaved Changes".into(),
            message,
            action: what,
            selected: 0,
        });
    }

    fn do_close(&mut self, what: PendingClose) {
        match what {
            PendingClose::Quit => {
                self.cancel_automation();
                self.agent.stop();
                self.quit = true;
            }
            PendingClose::Tab(i) => {
                if i < self.editors.len() {
                    self.editors.remove(i);
                    if self.active >= self.editors.len() {
                        self.active = self.editors.len().saturating_sub(1);
                    } else if i < self.active {
                        self.active -= 1;
                    }
                    if self.editors.is_empty() {
                        self.focus = Focus::Explorer;
                        self.find.visible = false;
                    }
                }
            }
        }
    }

    fn dialog_key(&mut self, code: KeyCode) {
        let Some(d) = &mut self.dialog else { return };
        let choice = match code {
            KeyCode::Left | KeyCode::BackTab => {
                d.selected = (d.selected + 2) % 3;
                None
            }
            KeyCode::Right | KeyCode::Tab => {
                d.selected = (d.selected + 1) % 3;
                None
            }
            KeyCode::Enter => Some(d.selected),
            KeyCode::Char('s') | KeyCode::Char('y') => Some(0),
            KeyCode::Char('d') | KeyCode::Char('n') => Some(1),
            KeyCode::Esc | KeyCode::Char('c') => Some(2),
            _ => None,
        };
        if let Some(c) = choice {
            self.dialog_choose(c);
        }
    }

    fn dialog_choose(&mut self, choice: usize) {
        let Some(d) = self.dialog.take() else { return };
        let targets: Vec<usize> = match d.action {
            PendingClose::Tab(i) => vec![i],
            PendingClose::Quit => (0..self.editors.len()).collect(),
        };
        match choice {
            0 => {
                for i in targets {
                    let e = &mut self.editors[i];
                    if e.doc.is_dirty() {
                        if e.doc.path.is_none() {
                            self.active = i;
                            self.open_palette(PaletteMode::SaveAs);
                            return;
                        }
                        if let Err(err) = e.doc.save() {
                            self.toast(format!("Save failed: {err:#}"), ToastKind::Error);
                            return;
                        }
                    }
                }
                self.do_close(d.action);
            }
            1 => self.do_close(d.action),
            _ => {}
        }
    }

    // ================================================================ palette

    pub fn open_palette(&mut self, mode: PaletteMode) {
        let mut plugin_cmds: Vec<(usize, usize)> = Vec::new();
        let (items, hints): (Vec<String>, Vec<String>) = match mode {
            PaletteMode::Commands => {
                let mut items: Vec<String> = COMMANDS.iter().map(|c| c.title.to_string()).collect();
                let mut hints: Vec<String> = COMMANDS.iter().map(|c| c.keys.to_string()).collect();
                for (pi, ci, title) in self.plugins.commands() {
                    items.push(title);
                    hints.push(
                        self.plugins
                            .plugins
                            .get(pi)
                            .and_then(|p| p.commands.get(ci))
                            .and_then(|c| c.keys.clone())
                            .unwrap_or_else(|| "plugin".to_string()),
                    );
                    plugin_cmds.push((pi, ci));
                }
                (items, hints)
            }
            PaletteMode::Plugins => {
                let mut items = vec![
                    "Plugins: Reload".to_string(),
                    "Plugins: Reinstall Bundled Plugins".to_string(),
                ];
                let mut hints = vec![
                    "re-read the plugin folders".to_string(),
                    "rewrite ~/.cache/termed/plugins".to_string(),
                ];
                if self.plugins.plugins.is_empty() {
                    items.push("No plugins found — press Enter to look again".to_string());
                    hints.push(String::new());
                }
                for p in &self.plugins.plugins {
                    items.push(format!(
                        "{} {}  —  {}",
                        if p.enabled { "[on]" } else { "[off]" },
                        p.name,
                        p.status()
                    ));
                    hints.push(p.dir.to_string_lossy().into_owned());
                }
                (items, hints)
            }
            PaletteMode::Files => {
                let root = self.explorer.root.clone();
                let files = self
                    .project_files
                    .get_or_insert_with(|| explorer::project_files(&root, 50_000))
                    .clone();
                let hints = files
                    .iter()
                    .map(|f| {
                        Path::new(f)
                            .parent()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    })
                    .collect();
                (files, hints)
            }
            PaletteMode::Themes => (
                theme::theme_names().iter().map(|s| s.to_string()).collect(),
                Vec::new(),
            ),
            PaletteMode::Languages => {
                let mut names: Vec<String> = highlight::syntax_set()
                    .syntaxes()
                    .iter()
                    .filter(|s| !s.hidden)
                    .map(|s| s.name.clone())
                    .collect();
                names.sort_by_key(|n| n.to_lowercase());
                names.dedup();
                (names, Vec::new())
            }
            PaletteMode::AgentModels => {
                // Free models first: they need no account.
                let mut models = self.agent.models.clone();
                models.sort_by_key(|(v, _)| !is_free_model(v));
                models
                    .into_iter()
                    .map(|(v, name)| {
                        let hint = if is_free_model(&v) {
                            format!("{name} · free")
                        } else {
                            name
                        };
                        (v, hint)
                    })
                    .unzip()
            }
            PaletteMode::AgentPicker => self
                .agent
                .available
                .iter()
                .map(|a| {
                    let hint = if a.builtin {
                        "built-in · provider configured in AI settings"
                    } else if a.free {
                        "free, no account"
                    } else {
                        "may need sign-in"
                    };
                    (a.name.clone(), hint.to_string())
                })
                .unzip(),
            PaletteMode::Settings
            | PaletteMode::AgentSettings
            | PaletteMode::SubAgents
            | PaletteMode::SubAgentSettings
            | PaletteMode::Skills
            | PaletteMode::SkillActions
            | PaletteMode::PluginActions
            | PaletteMode::PluginCommands
            | PaletteMode::Input
            | PaletteMode::ConfirmRemoveSkill
            | PaletteMode::ConfirmRemoveSubAgent => self.management_items(mode),
            PaletteMode::GotoLine | PaletteMode::SaveAs => (Vec::new(), Vec::new()),
        };
        let mut input = if mode == PaletteMode::Input {
            self.management
                .edit
                .as_ref()
                .map(|e| e.value.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
        if mode == PaletteMode::SaveAs
            && let Some(e) = self.editor()
        {
            input = match &e.doc.path {
                Some(p) => p
                    .strip_prefix(&self.explorer.root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .into_owned(),
                None => String::new(),
            };
        }
        let restore_theme = (mode == PaletteMode::Themes).then(|| self.theme.name.clone());
        let cursor = input.chars().count();
        self.palette = Some(Palette {
            mode,
            input_selected: mode == PaletteMode::Input && !input.is_empty(),
            input,
            cursor,
            items,
            hints,
            filtered: Vec::new(),
            selected: 0,
            scroll: 0,
            restore_theme,
            plugin_cmds,
        });
        self.refilter();
        if mode == PaletteMode::Themes
            && let Some(p) = &mut self.palette
            && let Some(i) = p
                .filtered
                .iter()
                .position(|s| p.items[s.index] == self.theme.name)
        {
            p.selected = i;
        }
        if mode == PaletteMode::Languages {
            let lang = self
                .editor()
                .map(|e| e.language().to_string())
                .unwrap_or_default();
            if let Some(p) = &mut self.palette
                && let Some(i) = p.filtered.iter().position(|s| p.items[s.index] == lang)
            {
                p.selected = i;
            }
        }
    }

    fn refilter(&mut self) {
        let Some(p) = &mut self.palette else { return };
        let paths = p.mode == PaletteMode::Files;
        p.filtered = fuzzy::rank(&p.input, &p.items, paths);
        p.filtered.truncate(500);
        p.selected = 0;
        p.scroll = 0;
        if p.mode == PaletteMode::GotoLine
            && let (Ok(n), Some(e)) = (
                p.input
                    .trim()
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .parse::<usize>(),
                self.editors.get_mut(self.active),
            )
        {
            // Live preview, like VS's Go To Line.
            e.goto_line(n.saturating_sub(1));
        }
    }

    fn palette_key(&mut self, code: KeyCode, ctrl: bool) {
        let Some(p) = &mut self.palette else { return };
        let n = p.filtered.len();
        let mut moved = false;
        if code == KeyCode::Char('a') && ctrl {
            p.input_selected = true;
            return;
        }
        if p.input_selected && matches!(code, KeyCode::Backspace | KeyCode::Delete) {
            p.input.clear();
            p.cursor = 0;
            p.input_selected = false;
            self.refilter();
            return;
        }
        if p.input_selected && matches!(code, KeyCode::Char(_)) && !ctrl {
            p.input.clear();
            p.cursor = 0;
            p.input_selected = false;
        }
        if matches!(
            code,
            KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End
        ) {
            p.input_selected = false;
        }
        match code {
            KeyCode::Esc => {
                if p.mode == PaletteMode::Input {
                    self.palette = None;
                    self.cancel_setting_edit();
                    return;
                }
                if let Some(t) = p.restore_theme.take() {
                    self.set_theme(&t);
                }
                self.palette = None;
                return;
            }
            KeyCode::Enter => {
                self.palette_accept();
                return;
            }
            KeyCode::Up => {
                if n > 0 {
                    p.selected = (p.selected + n - 1) % n;
                }
                moved = true;
            }
            KeyCode::Down => {
                if n > 0 {
                    p.selected = (p.selected + 1) % n;
                }
                moved = true;
            }
            KeyCode::PageUp => {
                p.selected = p.selected.saturating_sub(10);
                moved = true;
            }
            KeyCode::PageDown => {
                p.selected = (p.selected + 10).min(n.saturating_sub(1));
                moved = true;
            }
            KeyCode::Left => p.cursor = p.cursor.saturating_sub(1),
            KeyCode::Right => p.cursor = (p.cursor + 1).min(p.input.chars().count()),
            KeyCode::Home => p.cursor = 0,
            KeyCode::End => p.cursor = p.input.chars().count(),
            KeyCode::Backspace => {
                if ctrl {
                    p.input.clear();
                    p.cursor = 0;
                } else if p.cursor > 0 {
                    let b = byte_at(&p.input, p.cursor - 1);
                    p.input.remove(b);
                    p.cursor -= 1;
                }
                self.refilter();
                return;
            }
            KeyCode::Delete => {
                if p.cursor < p.input.chars().count() {
                    p.input.remove(byte_at(&p.input, p.cursor));
                }
                self.refilter();
                return;
            }
            KeyCode::Char(c) if !ctrl => {
                let b = byte_at(&p.input, p.cursor);
                p.input.insert(b, c);
                p.cursor += 1;
                self.refilter();
                return;
            }
            _ => {}
        }
        if moved
            && p.mode == PaletteMode::Themes
            && let Some(s) = p.filtered.get(p.selected)
        {
            let name = p.items[s.index].clone();
            self.set_theme(&name);
        }
    }

    fn palette_accept(&mut self) {
        let Some(p) = self.palette.take() else { return };
        let chosen = p.filtered.get(p.selected).map(|s| p.items[s.index].clone());
        match p.mode {
            PaletteMode::Commands => {
                if let Some(s) = p.filtered.get(p.selected) {
                    if let Some(cmd) = COMMANDS.get(s.index) {
                        self.run(cmd.cmd);
                    } else if let Some(&(pi, ci)) = p.plugin_cmds.get(s.index - COMMANDS.len()) {
                        self.run_plugin(pi, ci);
                    }
                }
            }
            PaletteMode::Plugins => {
                if let Some(s) = p.filtered.get(p.selected) {
                    match s.index {
                        0 => {
                            self.cancel_automation();
                            self.plugins.reload();
                        }
                        1 => {
                            self.cancel_automation();
                            if let Err(e) = plugin::install_bundled() {
                                self.toast(
                                    format!("Cannot install plugins: {e}"),
                                    ToastKind::Error,
                                );
                            }
                            self.plugins.reload();
                        }
                        i if i >= 2 => {
                            let pi = i - 2;
                            if self.plugins.plugins.get(pi).is_some() {
                                self.management.plugin = Some(pi);
                                self.open_palette(PaletteMode::PluginActions);
                                return;
                            }
                        }
                        _ => {}
                    }
                    // Stay in the manager so the new state is visible.
                    self.open_palette(PaletteMode::Plugins);
                }
            }
            PaletteMode::Settings
            | PaletteMode::AgentSettings
            | PaletteMode::SubAgents
            | PaletteMode::SubAgentSettings
            | PaletteMode::Skills
            | PaletteMode::SkillActions
            | PaletteMode::PluginActions
            | PaletteMode::PluginCommands
            | PaletteMode::Input
            | PaletteMode::ConfirmRemoveSkill
            | PaletteMode::ConfirmRemoveSubAgent => {
                self.management_accept(
                    p.mode,
                    p.filtered.get(p.selected).map(|s| s.index),
                    p.input,
                );
            }
            PaletteMode::Files => {
                if let Some(f) = chosen {
                    let path = self.explorer.root.join(f);
                    self.open_file(&path);
                }
            }
            PaletteMode::Themes => {
                if let Some(t) = chosen {
                    self.set_theme(&t);
                    self.persist_editor_preferences();
                }
            }
            PaletteMode::AgentModels => {
                if let Some(v) = chosen {
                    let mut prefs = self.preferences.clone();
                    prefs.agent.model = v.clone();
                    let saved = self.ensure_preferences_writable().and_then(|_| {
                        if self.agent.is_working() || self.automation.busy() {
                            anyhow::bail!("Stop the AI turn before changing its model");
                        }
                        prefs.save(&self.explorer.root)
                    });
                    if let Err(e) = saved {
                        self.toast(format!("Model not saved: {e:#}"), ToastKind::Error);
                    } else {
                        self.preferences = prefs;
                        self.agent.options.model = v.clone();
                        self.agent.set_model(&v);
                    }
                }
                self.focus = Focus::Agent;
            }
            PaletteMode::AgentPicker => {
                if let Some(spec) = p
                    .filtered
                    .get(p.selected)
                    .and_then(|s| self.agent.available.get(s.index))
                    .cloned()
                {
                    let mut prefs = self.preferences.clone();
                    let custom = std::env::var("TERMED_AGENT").ok().filter(|command| {
                        let args = crate::plugin::split_command(command);
                        args.first() == Some(&spec.program)
                            && args.get(1..) == Some(spec.args.as_slice())
                    });
                    if spec.builtin {
                        prefs.agent.backend = "builtin".into();
                    } else if let Some(command) = custom {
                        // A display name derived from TERMED_AGENT is not a
                        // discoverable backend after the environment changes.
                        // Persist the command itself for future launches.
                        prefs.agent.backend = "custom".into();
                        prefs.agent.command = command;
                    } else {
                        prefs.agent.backend = spec.name;
                    }
                    if let Err(e) = self.save_preferences(prefs) {
                        self.toast(format!("Agent settings: {e:#}"), ToastKind::Error);
                        return;
                    }
                    self.setup_bridge();
                    self.agent.start();
                }
                self.show_agent = true;
                self.focus = Focus::Agent;
            }
            PaletteMode::Languages => {
                if let (Some(name), Some(e)) = (chosen, self.editors.get_mut(self.active))
                    && let Some(s) = highlight::syntax_set().find_syntax_by_name(&name)
                {
                    e.hl.set_syntax(s);
                }
            }
            PaletteMode::GotoLine => {
                let mut parts = p.input.trim().split(':');
                let line = parts.next().and_then(|s| s.parse::<usize>().ok());
                let col = parts.next().and_then(|s| s.parse::<usize>().ok());
                if let (Some(l), Some(e)) = (line, self.editors.get_mut(self.active)) {
                    e.goto_line(l.saturating_sub(1));
                    if let Some(c) = col {
                        let (line, _) = e.cursor();
                        e.move_to(line, c.saturating_sub(1), false);
                    }
                }
                self.focus = Focus::Editor;
            }
            PaletteMode::SaveAs => {
                let input = p.input.trim().to_string();
                if input.is_empty() {
                    return;
                }
                let path = {
                    let raw = PathBuf::from(&input);
                    if raw.is_absolute() {
                        raw
                    } else {
                        self.explorer.root.join(raw)
                    }
                };
                let result = if let Some(e) = self.editors.get_mut(self.active) {
                    let export = e.doc.is_read_only();
                    e.doc.save_as(&path).map(|()| {
                        if !export {
                            let first = e.doc.line_str(0);
                            e.hl.set_syntax(highlight::detect_syntax(Some(&path), &first));
                        }
                        export
                    })
                } else {
                    return;
                };
                match result {
                    Ok(export) => self.toast(
                        format!(
                            "{} {}",
                            if export { "Exported" } else { "Saved" },
                            path.display()
                        ),
                        ToastKind::Success,
                    ),
                    Err(e) => {
                        self.toast(format!("Save As failed: {e:#}"), ToastKind::Error);
                        return;
                    }
                }
                self.project_files = None;
                self.explorer.refresh();
                self.explorer.reveal(&path);
            }
        }
    }

    // ================================================================ find

    fn find_field(&mut self) -> (&mut String, &mut usize) {
        if self.find.on_replace {
            (&mut self.find.replace, &mut self.find.replace_cursor)
        } else {
            (&mut self.find.query, &mut self.find.query_cursor)
        }
    }

    fn find_regex(&self) -> Option<Regex> {
        let f = &self.find;
        if f.query.is_empty() {
            return None;
        }
        let mut pat = if f.regex {
            f.query.clone()
        } else {
            regex::escape(&f.query)
        };
        if f.whole_word {
            pat = format!(r"\b(?:{pat})\b");
        }
        RegexBuilder::new(&pat)
            .case_insensitive(!f.case_sensitive)
            .multi_line(true)
            .build()
            .ok()
    }

    /// Recompute matches if the query, options or document changed.
    pub fn update_find(&mut self) {
        let Some(e) = self.editors.get(self.active) else {
            self.find.matches.clear();
            return;
        };
        let key = (
            self.find.query.clone(),
            self.find.case_sensitive,
            self.find.whole_word,
            self.find.regex,
            e.doc.version,
            self.active,
        );
        if self.find.key.as_ref() == Some(&key) {
            return;
        }
        self.find.key = Some(key);
        self.find.matches.clear();
        self.find.error = false;
        if self.find.query.is_empty() {
            self.find.current = None;
            return;
        }
        let Some(re) = self.find_regex() else {
            self.find.error = true;
            return;
        };
        let rope = &e.doc.rope;
        let text = rope.to_string();
        for m in re.find_iter(&text).take(100_000) {
            if m.start() == m.end() {
                continue;
            }
            self.find
                .matches
                .push((rope.byte_to_char(m.start()), rope.byte_to_char(m.end())));
        }
        let head = e.sel.start();
        self.find.current = self
            .find
            .matches
            .iter()
            .position(|(s, _)| *s >= head)
            .or((!self.find.matches.is_empty()).then_some(0));
    }

    fn find_jump(&mut self, forward: bool, from_selection_start: bool) {
        let Some(e) = self.editors.get_mut(self.active) else {
            return;
        };
        let m = &self.find.matches;
        if m.is_empty() {
            return;
        }
        let pos = if from_selection_start {
            e.sel.start()
        } else if forward {
            e.sel.end()
        } else {
            e.sel.start()
        };
        let idx = if forward {
            m.iter().position(|(s, _)| *s >= pos).unwrap_or(0)
        } else {
            m.iter().rposition(|(s, _)| *s < pos).unwrap_or(m.len() - 1)
        };
        let (s, en) = m[idx];
        e.sel = Selection::new(s, en);
        self.find.current = Some(idx);
        let (line, _) = e.cursor();
        if line < e.scroll_line || line >= e.scroll_line + e.view_height {
            e.center_on_cursor();
        }
        e.scroll_into_view();
    }

    fn find_key(&mut self, code: KeyCode, ctrl: bool, shift: bool, alt: bool) {
        match (code, ctrl, alt) {
            (KeyCode::Esc, ..) => {
                self.find.visible = false;
                self.focus = Focus::Editor;
                return;
            }
            (KeyCode::Char('c'), _, true) => self.find.case_sensitive = !self.find.case_sensitive,
            (KeyCode::Char('w'), _, true) => self.find.whole_word = !self.find.whole_word,
            (KeyCode::Char('r'), _, true) => self.find.regex = !self.find.regex,
            (KeyCode::Enter, true, _) | (KeyCode::Char('a'), _, true) if self.find.on_replace => {
                self.replace_all();
                return;
            }
            (KeyCode::Enter, ..) => {
                self.update_find();
                if self.find.on_replace {
                    self.replace_one();
                } else {
                    self.find_jump(!shift, false);
                }
                return;
            }
            (KeyCode::Tab, ..)
            | (KeyCode::BackTab, ..)
            | (KeyCode::Down, ..)
            | (KeyCode::Up, ..) => {
                if self.find.show_replace {
                    self.find.on_replace = !self.find.on_replace;
                }
                return;
            }
            _ => {
                let selected =
                    std::mem::take(&mut self.find.query_selected) && !self.find.on_replace;
                if selected
                    && matches!(
                        code,
                        KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete
                    )
                {
                    self.find.query.clear();
                    self.find.query_cursor = 0;
                    if !matches!(code, KeyCode::Char(_)) {
                        self.update_find();
                        return;
                    }
                }
                let (field, cur) = self.find_field();
                match code {
                    KeyCode::Left => *cur = cur.saturating_sub(1),
                    KeyCode::Right => *cur = (*cur + 1).min(field.chars().count()),
                    KeyCode::Home => *cur = 0,
                    KeyCode::End => *cur = field.chars().count(),
                    KeyCode::Backspace if ctrl => {
                        field.clear();
                        *cur = 0;
                    }
                    KeyCode::Backspace => {
                        if *cur > 0 {
                            let b = byte_at(field, *cur - 1);
                            field.remove(b);
                            *cur -= 1;
                        }
                    }
                    KeyCode::Delete => {
                        if *cur < field.chars().count() {
                            let b = byte_at(field, *cur);
                            field.remove(b);
                        }
                    }
                    KeyCode::Char(c) if !ctrl && !alt => {
                        let b = byte_at(field, *cur);
                        field.insert(b, c);
                        *cur += 1;
                    }
                    _ => return,
                }
                if self.find.on_replace {
                    return;
                }
            }
        }
        self.update_find();
        self.find_jump(true, true);
    }

    fn replace_one(&mut self) {
        if self.editor().is_some_and(|e| e.doc.is_read_only()) {
            self.toast(
                "Preview is read-only; open a text member to edit",
                ToastKind::Info,
            );
            return;
        }
        let Some(i) = self.find.current else { return };
        let Some(&(s, en)) = self.find.matches.get(i) else {
            return;
        };
        let replacement = self.expand_replacement(s, en);
        if let Some(e) = self.editors.get_mut(self.active) {
            if e.sel.start() == s && e.sel.end() == en {
                e.replace_range(s, en, &replacement);
            } else {
                e.sel = Selection::caret(s);
            }
        }
        self.update_find();
        self.find_jump(true, false);
    }

    fn expand_replacement(&self, s: usize, e: usize) -> String {
        if !self.find.regex {
            return self.find.replace.clone();
        }
        let (Some(re), Some(ed)) = (self.find_regex(), self.editor()) else {
            return self.find.replace.clone();
        };
        let text = ed.doc.rope.slice(s..e).to_string();
        re.replace(&text, self.find.replace.as_str()).into_owned()
    }

    fn replace_all(&mut self) {
        if self.editor().is_some_and(|e| e.doc.is_read_only()) {
            self.toast(
                "Preview is read-only; open a text member to edit",
                ToastKind::Info,
            );
            return;
        }
        self.update_find();
        let ranges = self.find.matches.clone();
        let n = ranges.len();
        if n == 0 {
            return;
        }
        if self.find.regex {
            let reps: Vec<String> = ranges
                .iter()
                .map(|(s, e)| self.expand_replacement(*s, *e))
                .collect();
            if let Some(e) = self.editors.get_mut(self.active) {
                for ((s, en), r) in ranges.iter().zip(reps.iter()).rev() {
                    e.replace_range(*s, *en, r);
                }
            }
        } else {
            let rep = self.find.replace.clone();
            if let Some(e) = self.editors.get_mut(self.active) {
                e.replace_all(&ranges, &rep);
            }
        }
        self.update_find();
        self.toast(format!("Replaced {n} occurrence(s)"), ToastKind::Success);
    }

    // ========================================================== find in files

    /// Open the project-wide search panel. A single-line selection becomes
    /// the query, like Visual Studio.
    pub fn open_search(&mut self, replace: bool) {
        let already_open = self.search.visible && self.focus == Focus::Search;
        self.find.visible = false;
        self.search.visible = true;
        self.search.replace = replace;
        if self.focus != Focus::Search {
            if let Some(e) = self.editor() {
                let text = e.selected_text();
                if !text.is_empty() && !text.contains(['\n', '\r']) && text.len() <= 200 {
                    self.search.query = text;
                    self.search.query_cursor = self.search.query.chars().count();
                    self.search.query_selected = true;
                }
            }
            self.search.field = SearchField::Query;
        }
        if !replace && self.search.field == SearchField::Replace {
            self.search.field = SearchField::Query;
        }
        // Ctrl+Shift+H on an open search moves on to the replacement.
        if already_open && replace && self.search.field == SearchField::Query {
            self.search.field = SearchField::Replace;
        }
        self.focus = Focus::Search;
        // A full-screen agent sheet must not obscure a newly opened search.
        if self.compact {
            self.show_agent = false;
            self.show_sidebar = false;
        }
        self.search.key = None;
        self.update_search();
    }

    fn close_search(&mut self) {
        self.search.visible = false;
        self.focus = if self.editors.is_empty() {
            Focus::Explorer
        } else {
            Focus::Editor
        };
    }

    fn search_field_mut(&mut self) -> (&mut String, &mut usize) {
        match self.search.field {
            SearchField::Replace if self.search.replace => (
                &mut self.search.replacement,
                &mut self.search.replace_cursor,
            ),
            SearchField::Root => (&mut self.search.root, &mut self.search.root_cursor),
            SearchField::Dirs => (&mut self.search.dirs, &mut self.search.dirs_cursor),
            SearchField::Filter => (&mut self.search.filter, &mut self.search.filter_cursor),
            _ => (&mut self.search.query, &mut self.search.query_cursor),
        }
    }

    fn search_query(&self) -> search::Query {
        search::Query {
            text: self.search.query.clone(),
            case_sensitive: self.search.case_sensitive,
            whole_word: self.search.whole_word,
            regex: self.search.regex,
        }
    }

    fn search_regex(&mut self) -> Option<Regex> {
        let q = self.search_query();
        if q.is_empty() {
            return None;
        }
        match q.compile() {
            Ok(re) => Some(re),
            Err(e) => {
                self.toast(format!("Invalid pattern: {e}"), ToastKind::Error);
                None
            }
        }
    }

    /// Recompute the results when the query, options, filter or an open
    /// editor changed. Cheap to call: nothing happens if the key is the same.
    pub fn update_search(&mut self) {
        if !self.search.visible {
            return;
        }
        let q = self.search_query();
        // Open editors can hold unsaved text; their edits are part of the key
        // so nothing is copied until the results are actually stale.
        let mut versions = self.editors.len() as u64;
        for e in &self.editors {
            if let Some(p) = &e.doc.path {
                let mut h = DefaultHasher::new();
                p.hash(&mut h);
                e.doc.version.hash(&mut h);
                versions = versions.rotate_left(9) ^ h.finish();
            }
        }
        let key = (
            q.text.clone(),
            self.search.filter.clone(),
            self.search.dirs.clone(),
            self.search.root.clone(),
            q.case_sensitive,
            q.whole_word,
            q.regex,
            versions,
            self.explorer.root.clone(),
        );
        if self.search.key.as_ref() == Some(&key) {
            return;
        }
        self.search.key = Some(key);
        let mut overrides = HashMap::new();
        for e in &self.editors {
            if !e.doc.is_read_only()
                && let Some(p) = &e.doc.path
            {
                overrides.insert(p.clone(), e.doc.rope.to_string());
            }
        }
        // Keep the selection on the same occurrence if it is still there.
        let prev = self.search.selected.and_then(|(fi, mi)| {
            let f = self.search.results.files.get(fi)?;
            let m = f.matches.get(mi)?;
            Some((f.path.clone(), m.line, m.start))
        });
        self.search.error = false;
        self.search.selected = None;
        self.search.scroll = 0;
        self.search.root_path = search::resolve_root(&self.explorer.root, &self.search.root);
        self.search.missing_root = !self.search.root_path.is_dir();
        if q.is_empty() {
            self.search.results = search::Results::default();
            return;
        }
        let root = search::resolve_root(&self.explorer.root, &self.search.root);
        self.search.root_path = root.clone();
        let filter = search::FilterSet::parse_parts(&self.search.dirs, &self.search.filter);
        self.search.missing_root = !root.is_dir();
        if self.search.missing_root {
            self.search.results = search::Results::default();
            return;
        }
        match search::search(&root, &q, &filter, &overrides) {
            Ok(results) => {
                self.search.results = results;
                self.search.selected = prev.and_then(|(path, line, start)| {
                    self.search
                        .results
                        .files
                        .iter()
                        .enumerate()
                        .find_map(|(fi, f)| {
                            if f.path != path {
                                return None;
                            }
                            f.matches
                                .iter()
                                .position(|m| m.line == line && m.start == start)
                                .map(|mi| (fi, mi))
                        })
                });
                if self.search.selected.is_none() {
                    self.search.selected = self.first_search_hit();
                }
            }
            Err(_) => {
                self.search.results = search::Results::default();
                self.search.error = true;
            }
        }
    }

    fn first_search_hit(&self) -> Option<(usize, usize)> {
        self.search
            .results
            .files
            .iter()
            .position(|f| !f.matches.is_empty())
            .map(|fi| (fi, 0))
    }

    /// The flat rows of the results list: a header per file, then its
    /// matches (unless the file is collapsed).
    pub fn search_rows(&self) -> Vec<(usize, Option<usize>)> {
        let mut rows = Vec::new();
        for (fi, f) in self.search.results.files.iter().enumerate() {
            rows.push((fi, None));
            if !self.search.collapsed.contains(&f.path) {
                rows.extend((0..f.matches.len()).map(|mi| (fi, Some(mi))));
            }
        }
        rows
    }

    fn search_hits(&self) -> Vec<(usize, usize)> {
        self.search
            .results
            .files
            .iter()
            .enumerate()
            .flat_map(|(fi, f)| (0..f.matches.len()).map(move |mi| (fi, mi)))
            .collect()
    }

    fn search_move(&mut self, delta: isize) {
        let hits = self.search_hits();
        if hits.is_empty() {
            return;
        }
        let n = hits.len() as isize;
        let next = match self
            .search
            .selected
            .and_then(|s| hits.iter().position(|h| *h == s))
        {
            Some(i) => (i as isize + delta).rem_euclid(n),
            None if delta < 0 => n - 1,
            None => 0,
        };
        let (fi, mi) = hits[next as usize];
        self.search.selected = Some((fi, mi));
        if let Some(f) = self.search.results.files.get(fi) {
            // Keyboard navigation should never land on a hidden row.
            self.search.collapsed.remove(&f.path);
        }
    }

    fn search_hit_valid(&self) -> bool {
        self.search.selected.is_some_and(|(fi, mi)| {
            self.search
                .results
                .files
                .get(fi)
                .is_some_and(|f| mi < f.matches.len())
        })
    }

    fn open_selected_search_hit(&mut self) {
        if !self.search_hit_valid() {
            self.search.selected = self.first_search_hit();
        }
        match self.search.selected {
            Some((fi, mi)) => self.open_search_hit(fi, mi),
            None => self.update_search(),
        }
    }

    /// Jump to a result: open its file and select the occurrence.
    pub fn open_search_hit(&mut self, fi: usize, mi: usize) {
        let Some(f) = self.search.results.files.get(fi) else {
            return;
        };
        let Some(m) = f.matches.get(mi) else {
            return;
        };
        let (path, line, start, end) = (f.path.clone(), m.line, m.start, m.end);
        self.search.selected = Some((fi, mi));
        self.open_file(&path);
        if let Some(e) = self.editor_mut() {
            let line = line.min(e.doc.len_lines().saturating_sub(1));
            let len = e.doc.line_len(line);
            let s = e.doc.idx(line, start.min(len));
            let en = e.doc.idx(line, end.min(len));
            e.sel = Selection::new(s, en);
            e.center_on_cursor();
            e.scroll_into_view();
        }
        // Keep typing in the search box; Esc goes back to the editor.
        self.focus = Focus::Search;
    }

    fn toggle_search_file(&mut self, fi: usize) {
        let Some(path) = self.search.results.files.get(fi).map(|f| f.path.clone()) else {
            return;
        };
        if !self.search.collapsed.remove(&path) {
            self.search.collapsed.insert(path);
        }
    }

    /// Replace in one path: an open editor keeps the change in memory (so it
    /// can be undone and saved); a closed file is written to disk.
    fn replace_in_path(
        &mut self,
        path: &Path,
        re: &Regex,
        replacement: &str,
        literal: bool,
    ) -> Result<usize, ()> {
        if let Some(i) = self
            .editors
            .iter()
            .position(|e| e.doc.path.as_deref() == Some(path))
        {
            if self.editors[i].doc.is_read_only() {
                return Err(());
            }
            let text = self.editors[i].doc.rope.to_string();
            let (new, n) = search::replace_in_text(&text, re, replacement, literal);
            if n > 0 {
                self.editors[i].replace_document(&new);
            }
            return Ok(n);
        }
        let file = search::TextFile::read(path).ok_or(())?;
        let (new, n) = search::replace_in_text(&file.text, re, replacement, literal);
        if n == 0 {
            return Ok(0);
        }
        file.write(&new).map_err(|_| ())?;
        Ok(n)
    }

    fn after_search_replace(&mut self, total: usize, files: usize, failed: usize) {
        if total == 0 {
            let (text, kind) = if failed > 0 {
                (
                    "Could not replace: some files could not be read".to_string(),
                    ToastKind::Error,
                )
            } else {
                ("No matches to replace".to_string(), ToastKind::Info)
            };
            self.toast(text, kind);
            return;
        }
        self.search.key = None;
        self.update_search();
        let mut msg = format!("Replaced {total} occurrence(s) in {files} file(s)");
        if failed > 0 {
            msg.push_str(&format!(" · {failed} failed"));
        }
        self.toast(
            msg,
            if failed > 0 {
                ToastKind::Error
            } else {
                ToastKind::Success
            },
        );
    }

    fn search_replace_all(&mut self) {
        let Some(re) = self.search_regex() else {
            return;
        };
        let literal = !self.search.regex;
        let replacement = self.search.replacement.clone();
        let paths: Vec<PathBuf> = self
            .search
            .results
            .files
            .iter()
            .map(|f| f.path.clone())
            .collect();
        let (mut total, mut files, mut failed) = (0, 0, 0);
        for path in paths {
            match self.replace_in_path(&path, &re, &replacement, literal) {
                Ok(0) => {}
                Ok(n) => {
                    total += n;
                    files += 1;
                }
                Err(()) => failed += 1,
            }
        }
        self.after_search_replace(total, files, failed);
    }

    fn search_replace_file(&mut self) {
        if !self.search_hit_valid() {
            self.toast("Select a result first", ToastKind::Info);
            return;
        }
        let Some((fi, _)) = self.search.selected else {
            return;
        };
        let Some(path) = self.search.results.files.get(fi).map(|f| f.path.clone()) else {
            return;
        };
        let Some(re) = self.search_regex() else {
            return;
        };
        let literal = !self.search.regex;
        let replacement = self.search.replacement.clone();
        match self.replace_in_path(&path, &re, &replacement, literal) {
            Ok(n) => self.after_search_replace(n, if n > 0 { 1 } else { 0 }, 0),
            Err(()) => self.after_search_replace(0, 0, 1),
        }
    }

    /// Replace just the selected occurrence.
    pub fn search_replace_one(&mut self, fi: usize, mi: usize) {
        let Some(re) = self.search_regex() else {
            return;
        };
        let literal = !self.search.regex;
        let replacement = self.search.replacement.clone();
        let Some((path, line, start)) = self.search.results.files.get(fi).and_then(|f| {
            let m = f.matches.get(mi)?;
            Some((f.path.clone(), m.line, m.start))
        }) else {
            return;
        };
        let mut done = false;
        if let Some(i) = self
            .editors
            .iter()
            .position(|e| e.doc.path.as_deref() == Some(path.as_path()))
        {
            let e = &mut self.editors[i];
            if line < e.doc.len_lines() {
                let text = e.doc.line_str(line);
                if let Some(new_line) = search::replace_at(&text, start, &re, &replacement, literal)
                {
                    let s = e.doc.line_start(line);
                    let len = e.doc.line_len(line);
                    e.replace_range(s, s + len, &new_line);
                    done = true;
                }
            }
        } else if let Some(file) = search::TextFile::read(&path) {
            let mut out = String::with_capacity(file.text.len());
            for (n, l) in file.text.split('\n').enumerate() {
                if n > 0 {
                    out.push('\n');
                }
                if n == line && !done {
                    match search::replace_at(l, start, &re, &replacement, literal) {
                        Some(new_line) => {
                            out.push_str(&new_line);
                            done = true;
                        }
                        None => out.push_str(l),
                    }
                } else {
                    out.push_str(l);
                }
            }
            if done && file.write(&out).is_err() {
                done = false;
            }
        }
        if done {
            self.search.key = None;
            self.update_search();
            self.toast("Replaced 1 occurrence", ToastKind::Success);
        } else {
            self.toast("That match is gone — search again", ToastKind::Info);
        }
    }

    fn search_replace_one_selected(&mut self) {
        if !self.search_hit_valid() {
            self.toast("Select a result first", ToastKind::Info);
            return;
        }
        if let Some((fi, mi)) = self.search.selected {
            self.search_replace_one(fi, mi);
        }
    }

    fn search_button(&mut self, b: u8) {
        match b {
            0 => self.search.case_sensitive = !self.search.case_sensitive,
            1 => self.search.whole_word = !self.search.whole_word,
            2 => self.search.regex = !self.search.regex,
            3 => {
                self.search_replace_all();
                return;
            }
            4 => {
                self.search_replace_file();
                return;
            }
            5 => {
                self.close_search();
                return;
            }
            _ => return,
        }
        self.update_search();
    }

    fn cycle_search_field(&mut self, back: bool) {
        let mut fields = vec![
            SearchField::Query,
            SearchField::Root,
            SearchField::Dirs,
            SearchField::Filter,
        ];
        if self.search.replace {
            fields.insert(1, SearchField::Replace);
        }
        let i = fields
            .iter()
            .position(|f| *f == self.search.field)
            .unwrap_or(0);
        let n = fields.len();
        self.search.field = fields[if back { (i + n - 1) % n } else { (i + 1) % n }];
    }

    fn search_key(&mut self, code: KeyCode, ctrl: bool, shift: bool, alt: bool) {
        match (code, ctrl, shift, alt) {
            (KeyCode::Esc, ..) => {
                self.close_search();
                return;
            }
            (KeyCode::Tab, ..) | (KeyCode::BackTab, ..) => {
                self.cycle_search_field(code == KeyCode::BackTab || shift);
                return;
            }
            (KeyCode::Enter, true, true, _) => {
                if self.search.replace {
                    self.search_replace_one_selected();
                }
                return;
            }
            (KeyCode::Enter, true, false, _) => {
                if self.search.replace {
                    self.search_replace_all();
                } else {
                    self.open_selected_search_hit();
                }
                return;
            }
            (KeyCode::Enter, false, false, true) => {
                if self.search.replace {
                    self.search_replace_file();
                }
                return;
            }
            (KeyCode::Enter, false, false, false) => {
                if self.search.replace && self.search.field == SearchField::Replace {
                    self.search_replace_one_selected();
                } else {
                    self.open_selected_search_hit();
                }
                return;
            }
            (KeyCode::F(5), ..) => {
                // Re-run the search after editing files behind the panel.
                self.search.key = None;
                self.update_search();
                return;
            }
            (KeyCode::Up, ..) => {
                self.search_move(-1);
                return;
            }
            (KeyCode::Down, ..) => {
                self.search_move(1);
                return;
            }
            (KeyCode::PageUp, ..) => {
                self.search_move(-10);
                return;
            }
            (KeyCode::PageDown, ..) => {
                self.search_move(10);
                return;
            }
            (KeyCode::Char('c'), _, _, true) => {
                self.search.case_sensitive = !self.search.case_sensitive;
                self.update_search();
                return;
            }
            (KeyCode::Char('w'), _, _, true) => {
                self.search.whole_word = !self.search.whole_word;
                self.update_search();
                return;
            }
            (KeyCode::Char('r'), _, _, true) => {
                self.search.regex = !self.search.regex;
                self.update_search();
                return;
            }
            _ => {
                // Field editing.
                let selected = self.search.field == SearchField::Query
                    && std::mem::take(&mut self.search.query_selected);
                if selected
                    && matches!(
                        code,
                        KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete
                    )
                {
                    self.search.query.clear();
                    self.search.query_cursor = 0;
                    if !matches!(code, KeyCode::Char(_)) {
                        self.update_search();
                        return;
                    }
                }
                let (field, cur) = self.search_field_mut();
                match code {
                    KeyCode::Left => *cur = cur.saturating_sub(1),
                    KeyCode::Right => *cur = (*cur + 1).min(field.chars().count()),
                    KeyCode::Home => *cur = 0,
                    KeyCode::End => *cur = field.chars().count(),
                    KeyCode::Backspace if ctrl => {
                        field.clear();
                        *cur = 0;
                    }
                    KeyCode::Backspace => {
                        if *cur > 0 {
                            let b = byte_at(field, *cur - 1);
                            field.remove(b);
                            *cur -= 1;
                        }
                    }
                    KeyCode::Delete => {
                        if *cur < field.chars().count() {
                            let b = byte_at(field, *cur);
                            field.remove(b);
                        }
                    }
                    KeyCode::Char(c) if !ctrl && !alt => {
                        let b = byte_at(field, *cur);
                        field.insert(b, c);
                        *cur += 1;
                    }
                    _ => return,
                }
                self.update_search();
            }
        }
    }

    // ================================================================ plugins

    /// Start a plugin command (palette entry or keybinding).
    pub fn run_plugin(&mut self, pi: usize, ci: usize) {
        if self.editor().is_some_and(|e| e.doc.is_read_only()) {
            self.toast(
                "Binary previews are read-only; open a text member to run editor plugins",
                ToastKind::Error,
            );
            return;
        }
        let file = self.editor().and_then(|e| e.doc.path.clone());
        if let Err(e) = self.plugins.run_request(pi, ci, file.as_deref(), json!({})) {
            self.toast(e, ToastKind::Error);
            return;
        }
        let name = self
            .plugins
            .plugins
            .get(pi)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        if let Some(err) = self.plugins.start_errors.get(&pi).cloned() {
            self.toast(format!("Could not start {name}: {err}"), ToastKind::Error);
        } else if !self.plugins.running_plugin(pi) {
            self.toast(format!("{name} is not running"), ToastKind::Error);
        }
    }

    /// Answer a plugin's request. Anything unrecognized returns `null` so a
    /// newer plugin degrades instead of hanging.
    fn plugin_call(&mut self, method: &str, params: &Value) -> Value {
        match method {
            "editor.text" => match self.editor() {
                Some(e) => json!(e.doc.rope.to_string()),
                None => Value::Null,
            },
            "editor.path" => match self.editor().and_then(|e| e.doc.path.clone()) {
                Some(p) => json!(p.to_string_lossy()),
                None => Value::Null,
            },
            "editor.selection" => match self.editor() {
                Some(e) => json!(e.selected_text()),
                None => Value::Null,
            },
            "editor.insert" => {
                if let Some(text) = params.get("text").and_then(Value::as_str)
                    && let Some(e) = self.editor_mut()
                {
                    e.insert_text(text);
                    e.scroll_into_view();
                }
                Value::Null
            }
            "editor.replace" => {
                if let Some(text) = params.get("text").and_then(Value::as_str)
                    && let Some(e) = self.editor_mut()
                {
                    e.replace_document(text);
                }
                Value::Null
            }
            "editor.open" => {
                if let Some(path) = params.get("path").and_then(Value::as_str) {
                    let p = Path::new(path);
                    self.open_file(&if p.is_absolute() {
                        p.to_path_buf()
                    } else {
                        self.explorer.root.join(p)
                    });
                    if let Some(line) = params.get("line").and_then(Value::as_u64)
                        && let Some(e) = self.editor_mut()
                    {
                        let line = (line as usize).saturating_sub(1);
                        let at = e.doc.idx(line, 0);
                        e.sel = Selection::caret(at);
                        e.center_on_cursor();
                        e.scroll_into_view();
                    }
                }
                Value::Null
            }
            "project.root" => json!(self.explorer.root.to_string_lossy()),
            "project.files" => {
                let limit = params
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(20_000) as usize;
                let files = explorer::project_files(&self.explorer.root, limit);
                json!(files)
            }
            "ui.toast" => {
                let message = params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let kind = match params.get("kind").and_then(Value::as_str) {
                    Some("error") => ToastKind::Error,
                    Some("success") | Some("ok") => ToastKind::Success,
                    _ => ToastKind::Info,
                };
                if !message.is_empty() {
                    self.toast(message, kind);
                }
                Value::Null
            }
            _ => Value::Null,
        }
    }

    /// Drain plugin messages and answer their requests. Returns true when
    /// anything happened (the caller redraws).
    pub fn pump_plugins(&mut self) -> bool {
        let msgs = self.plugins.pump();
        if msgs.is_empty() {
            return false;
        }
        for msg in msgs {
            match msg {
                plugin::Msg::Request {
                    plugin,
                    id,
                    method,
                    params,
                } => {
                    let context = self.plugin_context(plugin);
                    let ai_call = self.automation.plugins.values().find(|c| c.pi == plugin);
                    let expects_buffer =
                        ai_call.is_some_and(|c| c.file.is_some() || c.untitled.is_some());
                    if method.starts_with("editor.") && ai_call.is_some() && context.is_none() {
                        if expects_buffer {
                            self.plugins.stop(plugin);
                            self.plugin_exited(plugin);
                        } else if let Some(id) = id {
                            self.plugins.reply(plugin, id, Value::Null);
                        }
                        continue;
                    }
                    let (active, focus) = (self.active, self.focus);
                    if let Some(index) = context {
                        self.active = index;
                    }
                    if method == "ui.toast" {
                        if let Some(text) = params.get("message").and_then(Value::as_str) {
                            for call in self
                                .automation
                                .plugins
                                .values_mut()
                                .filter(|c| c.pi == plugin)
                            {
                                if call.messages.len() < 20 {
                                    call.messages.push(text.chars().take(2000).collect());
                                }
                                if params.get("kind").and_then(Value::as_str) == Some("error") {
                                    call.error = Some(text.chars().take(2000).collect());
                                }
                            }
                        }
                    }
                    let result = self.plugin_call(&method, &params);
                    if context.is_some() {
                        self.active = active.min(self.editors.len().saturating_sub(1));
                        self.focus = focus;
                    }
                    if let Some(id) = id {
                        self.plugins.reply(plugin, id, result);
                    }
                }
                plugin::Msg::Response { plugin, id, result } => {
                    self.plugin_finished(plugin, id, result)
                }
                plugin::Msg::Exited { plugin, code } => {
                    self.plugin_exited(plugin);
                    if let Some(0) | None = code {
                        continue;
                    }
                    let name = self
                        .plugins
                        .plugins
                        .get(plugin)
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| "plugin".to_string());
                    self.toast(
                        format!("{name} stopped unexpectedly ({}).", code.unwrap_or(-1)),
                        ToastKind::Error,
                    );
                }
                plugin::Msg::Junk { .. } => {}
            }
        }
        true
    }

    // ================================================================ mouse

    fn on_mouse(&mut self, m: MouseEvent) {
        let pos = Position::new(m.column, m.row);
        let l = self.layout.clone();
        match m.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let d: isize = if m.kind == MouseEventKind::ScrollDown {
                    3
                } else {
                    -3
                };
                if self.palette.is_some() {
                    if let Some(p) = &mut self.palette {
                        let n = p.filtered.len();
                        p.selected = (p.selected as isize + d.signum())
                            .clamp(0, n.saturating_sub(1) as isize)
                            as usize;
                    }
                } else if self.search.visible && l.search_box.contains(pos) {
                    let rows = self.search_rows().len();
                    let max = rows.saturating_sub(l.search_list.height as usize);
                    self.search.scroll =
                        (self.search.scroll as isize + d).clamp(0, max as isize) as usize;
                } else if l.agent_panel.contains(pos) {
                    let a = &mut self.agent;
                    a.scroll = (a.scroll as isize - d).max(0) as usize;
                } else if l.sidebar.contains(pos) {
                    let max = self
                        .explorer
                        .entries
                        .len()
                        .saturating_sub(l.sidebar_list.height as usize);
                    self.explorer.scroll =
                        (self.explorer.scroll as isize + d).clamp(0, max as isize) as usize;
                } else if let Some(e) = self.editor_mut() {
                    if let Some(crate::document::Preview::Image(image)) = e.doc.preview.as_mut() {
                        image.zoom_by(if d > 0 { -1 } else { 1 });
                        return;
                    }
                    if m.modifiers.contains(KeyModifiers::SHIFT) {
                        e.scroll_col = (e.scroll_col as isize + d * 2).max(0) as usize;
                    } else {
                        e.scroll_by(d);
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) => self.mouse_down(pos, m.modifiers),
            MouseEventKind::Drag(MouseButton::Left) => self.mouse_drag(pos),
            MouseEventKind::Up(_) => self.drag = None,
            MouseEventKind::Down(MouseButton::Middle) => {
                if let Some((_, _, i)) = l.tabs.iter().find(|(r, _, _)| r.contains(pos)) {
                    self.request_close(PendingClose::Tab(*i));
                }
            }
            _ => {}
        }
    }

    fn click_count(&mut self, pos: Position) -> u8 {
        let now = Instant::now();
        let count = match self.last_click {
            Some((t, x, y, c))
                if now.duration_since(t) < Duration::from_millis(400)
                    && x == pos.x
                    && y == pos.y =>
            {
                c % 3 + 1
            }
            _ => 1,
        };
        self.last_click = Some((now, pos.x, pos.y, count));
        count
    }

    pub fn run_tool(&mut self, tool: Tool) {
        match tool {
            Tool::Cmd(c) => {
                self.run(c);
                self.mods.spend();
            }
            Tool::Key(k) => self.press(k),
            Tool::Mod(m) => self.mods.tap(m),
            Tool::BoxMode => {
                if self.mods.box_mode() {
                    self.mods.alt = Latch::Off;
                    self.mods.shift = Latch::Off;
                } else {
                    self.mods.alt = Latch::Locked;
                    self.mods.shift = Latch::Locked;
                    if !self.editors.is_empty() {
                        self.focus = Focus::Editor;
                    }
                }
            }
            Tool::Page => self.toolbar_page ^= 1,
        }
    }

    fn mouse_down(&mut self, pos: Position, mods: KeyModifiers) {
        let l = self.layout.clone();
        // The touch toolbar works on top of everything (it is how you press
        // Esc or arrows in a dialog or palette on a phone).
        if let Some((_, tool)) = l.toolbar.iter().find(|(r, _)| r.contains(pos)) {
            self.run_tool(*tool);
            return;
        }
        if let Some(d) = &self.dialog {
            let _ = d;
            if let Some((_, i)) = l.dialog_buttons.iter().find(|(r, _)| r.contains(pos)) {
                self.dialog_choose(*i);
            }
            return;
        }
        if self.palette.is_some() {
            if l.palette_list.contains(pos) {
                let row = (pos.y - l.palette_list.y) as usize;
                if let Some(p) = &mut self.palette {
                    let i = p.scroll + row;
                    if i < p.filtered.len() {
                        p.selected = i;
                        self.palette_accept();
                    }
                }
            } else {
                if let Some(t) = self.palette.as_mut().and_then(|p| p.restore_theme.take()) {
                    self.set_theme(&t);
                }
                self.palette = None;
            }
            return;
        }
        // Tapping outside the explorer drawer closes it.
        if l.drawer.width > 0
            && !l.drawer.contains(pos)
            && !l.click_items.iter().any(|(r, _)| r.contains(pos))
        {
            self.show_sidebar = false;
            if self.focus == Focus::Explorer && !self.editors.is_empty() {
                self.focus = Focus::Editor;
            }
            return;
        }
        if l.agent_panel.contains(pos) {
            if let Some((_, t)) = l.agent_targets.iter().find(|(r, _)| r.contains(pos)) {
                self.agent_target(t.clone());
            } else {
                self.focus = Focus::Agent;
                if l.agent_input.contains(pos) {
                    // Tap in the input: move the caret to the end (simple and predictable on touch).
                    self.agent.cursor = self.agent.input.chars().count();
                }
            }
            return;
        }
        if self.search.visible && l.search_box.contains(pos) {
            if let Some((_, b)) = l.search_buttons.iter().find(|(r, _)| r.contains(pos)) {
                self.search_button(*b);
                return;
            }
            if let Some((_, f)) = l.search_fields.iter().find(|(r, _)| r.contains(pos)) {
                self.focus = Focus::Search;
                self.search.field = match f {
                    0 => SearchField::Query,
                    1 => SearchField::Replace,
                    2 => SearchField::Root,
                    3 => SearchField::Dirs,
                    _ => SearchField::Filter,
                };
                self.update_search();
                return;
            }
            if let Some((_, (fi, mi))) = l.search_replace_hits.iter().find(|(r, _)| r.contains(pos))
            {
                self.search_replace_one(*fi, *mi);
                return;
            }
            if let Some((_, fi)) = l.search_files.iter().find(|(r, _)| r.contains(pos)) {
                self.toggle_search_file(*fi);
                return;
            }
            if let Some((_, (fi, mi))) = l.search_hits.iter().find(|(r, _)| r.contains(pos)) {
                self.open_search_hit(*fi, *mi);
                return;
            }
            self.focus = Focus::Search;
            self.update_search();
            return;
        }
        if l.title_search.contains(pos) {
            self.run(Command::QuickOpen);
            return;
        }
        if let Some((_, cmd)) = l.click_items.iter().find(|(r, _)| r.contains(pos)) {
            self.run(*cmd);
            return;
        }
        if let Some((_, b)) = l.find_buttons.iter().find(|(r, _)| r.contains(pos)) {
            match b {
                0 => self.find.case_sensitive = !self.find.case_sensitive,
                1 => self.find.whole_word = !self.find.whole_word,
                2 => self.find.regex = !self.find.regex,
                3 => {
                    self.update_find();
                    self.find_jump(false, false);
                }
                4 => {
                    self.update_find();
                    self.find_jump(true, false);
                }
                5 => {
                    self.find.visible = false;
                    self.focus = Focus::Editor;
                }
                6 => self.replace_one(),
                7 => self.replace_all(),
                8 => self.find.show_replace = !self.find.show_replace,
                _ => {}
            }
            self.update_find();
            return;
        }
        if l.find_box.contains(pos) {
            self.focus = Focus::Find;
            self.find.on_replace = self.find.show_replace && pos.y > l.find_box.y + 1;
            return;
        }
        for (r, close, i) in &l.tabs {
            if close.contains(pos) {
                self.request_close(PendingClose::Tab(*i));
                return;
            }
            if r.contains(pos) {
                self.active = *i;
                self.focus = Focus::Editor;
                return;
            }
        }
        if l.sidebar.width > 0
            && pos.x == l.sidebar.x + l.sidebar.width - 1
            && l.sidebar.contains(pos)
        {
            self.drag = Some(Drag::SidebarResize);
            return;
        }
        if l.sidebar_list.contains(pos) {
            self.focus = Focus::Explorer;
            let i = self.explorer.scroll + (pos.y - l.sidebar_list.y) as usize;
            if i < self.explorer.entries.len() {
                self.explorer.selected = i;
                self.activate_explorer_entry();
                if !self.explorer.entries
                    [self.explorer.selected.min(self.explorer.entries.len() - 1)]
                .is_dir
                {
                    self.focus = Focus::Editor;
                }
            }
            return;
        }
        if l.sidebar.contains(pos) {
            self.focus = Focus::Explorer;
            return;
        }
        if l.scrollbar.contains(pos) {
            self.drag = Some(Drag::Scrollbar);
            self.mouse_drag(pos);
            return;
        }
        if l.minimap.contains(pos) {
            self.drag = Some(Drag::Minimap);
            self.mouse_drag(pos);
            return;
        }
        if l.text.contains(pos) || l.gutter.contains(pos) {
            self.focus = Focus::Editor;
            if self
                .editor()
                .is_some_and(|e| matches!(e.doc.preview, Some(crate::document::Preview::Image(_))))
            {
                return;
            }
            let count = self.click_count(pos);
            let in_gutter = l.gutter.contains(pos);
            let Some(idx) = self.idx_at(pos) else { return };
            let shift = mods.contains(KeyModifiers::SHIFT) || self.mods.shift.on();
            let alt = mods.contains(KeyModifiers::ALT) || self.mods.alt.on();
            let point = self.point_at(pos);
            self.mods.spend();
            let Some(e) = self.editor_mut() else { return };
            if alt && !in_gutter {
                // Alt+click / Alt+drag (or a tap in box mode): box selection
                // from the existing box anchor, or from the caret.
                if let Some((line, col)) = point {
                    let _ = (shift, count);
                    let (al, ac) = match e.block {
                        Some(b) => (b.anchor_line, b.anchor_col),
                        None => (e.cursor().0, e.cursor_display_col()),
                    };
                    e.box_to(al, ac, line, col);
                }
                self.drag = Some(Drag::Box);
                return;
            }
            e.box_clear();
            if in_gutter {
                let (line, _) = e.doc.pos(idx);
                let start = e.doc.line_start(line);
                let end = if line + 1 < e.doc.len_lines() {
                    e.doc.line_start(line + 1)
                } else {
                    e.doc.rope.len_chars()
                };
                e.sel = Selection::new(start, end);
            } else {
                match count {
                    2 => e.select_word_at(idx),
                    3 => {
                        e.sel = Selection::caret(idx);
                        e.select_line();
                    }
                    _ => {
                        e.sel = if shift {
                            Selection::new(e.sel.anchor, idx)
                        } else {
                            Selection::caret(idx)
                        };
                    }
                }
            }
            e.preferred_col = None;
            self.drag = Some(Drag::Text);
        }
    }

    fn mouse_drag(&mut self, pos: Position) {
        let l = self.layout.clone();
        match self.drag {
            Some(Drag::Text) => {
                // Auto-scroll when dragging past the edges.
                if let Some(e) = self.editor_mut() {
                    if pos.y < l.text.y {
                        e.scroll_by(-1);
                    } else if pos.y >= l.text.y + l.text.height {
                        e.scroll_by(1);
                    }
                }
                let clamped = Position::new(
                    pos.x
                        .clamp(l.text.x, l.text.x + l.text.width.saturating_sub(1)),
                    pos.y
                        .clamp(l.text.y, l.text.y + l.text.height.saturating_sub(1)),
                );
                if let Some(idx) = self.idx_at(clamped)
                    && let Some(e) = self.editor_mut()
                {
                    e.sel = Selection::new(e.sel.anchor, idx);
                }
            }
            Some(Drag::Box) => {
                let clamped = Position::new(
                    pos.x
                        .clamp(l.text.x, l.text.x + l.text.width.saturating_sub(1)),
                    pos.y
                        .clamp(l.text.y, l.text.y + l.text.height.saturating_sub(1)),
                );
                if let Some((line, col)) = self.point_at(clamped)
                    && let Some(e) = self.editor_mut()
                    && let Some(b) = e.block
                {
                    e.box_to(b.anchor_line, b.anchor_col, line, col);
                }
            }
            Some(Drag::Scrollbar) | Some(Drag::Minimap) => {
                let area = if self.drag == Some(Drag::Scrollbar) {
                    l.scrollbar
                } else {
                    l.minimap
                };
                if let Some(e) = self.editor_mut() {
                    let total = e.doc.len_lines();
                    let rel = (pos.y.saturating_sub(area.y)) as f32 / area.height.max(1) as f32;
                    let target = (rel * total as f32) as usize;
                    let half = e.view_height / 2;
                    e.scroll_to_line(target.saturating_sub(half));
                }
            }
            Some(Drag::SidebarResize) => {
                self.sidebar_width = pos.x.saturating_sub(l.sidebar.x).clamp(16, 80);
            }
            None => {}
        }
    }

    /// (line, display column) under a screen position; the column may lie
    /// past the end of the line (virtual space, for box selection).
    fn point_at(&self, pos: Position) -> Option<(usize, usize)> {
        let l = &self.layout;
        let e = self.editor()?;
        let row = pos.y.checked_sub(l.text.y)? as usize;
        let x = pos.x.saturating_sub(l.text.x) as usize + e.scroll_col;
        let rows = e.visual_rows(row + 1);
        let &(line, _, seg, _) = rows.get(row).or(rows.last())?;
        Some((line, seg.dstart + x.saturating_sub(seg.indent)))
    }

    /// Document char index under a screen position in the text area.
    fn idx_at(&self, pos: Position) -> Option<usize> {
        let l = &self.layout;
        let e = self.editor()?;
        let row = pos.y.checked_sub(l.text.y)? as usize;
        let x = pos.x.saturating_sub(l.text.x) as usize + e.scroll_col;
        let rows = e.visual_rows(row + 1);
        // Below the last line: end of document.
        let Some(&(line, _, seg, last)) = rows.get(row) else {
            return Some(e.doc.rope.len_chars());
        };
        let col = e.col_at_x(line, &seg, last, x);
        Some(e.doc.idx(line, col))
    }
}

/// Recognize provider model IDs explicitly marked as free.
pub fn is_free_model(value: &str) -> bool {
    value.contains("free")
}

impl App {
    // ================================================================ agent

    /// Drain agent messages and act on them. Returns true if anything changed.
    pub fn pump_agent(&mut self) -> bool {
        if self.automation.busy() {
            // External ACP agents can wait on MCP children/plugins without
            // sending transcript updates. Do not replay their prompt while
            // the live host task is still running; overwatch owns its deadline.
            self.agent.host_progress();
        }
        let (events, changed) = self.agent.poll();
        for event in events {
            self.process_agent_event(Source::Main, event);
        }
        self.pump_automation() || changed
    }

    /// Keep animating while the agent works (spinners, elapsed time).
    pub fn agent_animating(&self) -> bool {
        self.agent.is_working() || self.automation.busy()
    }

    /// An agent changed `path` on disk: show it in the editor as an undoable
    /// change with change bars, so you can review it (and Ctrl+Z it).
    pub(crate) fn agent_file_changed(&mut self, path: &Path, diffs: &[(Option<String>, String)]) {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let Ok(new) = std::fs::read_to_string(&path) else {
            return; // deleted or moved: the explorer refresh at the end of the turn shows it
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(e) = self
            .editors
            .iter_mut()
            .find(|e| e.doc.path.as_deref() == Some(path.as_path()))
        {
            if e.doc.is_dirty() {
                self.toast(
                    format!("{name} changed on disk; your unsaved edits were kept"),
                    ToastKind::Error,
                );
                return;
            }
            e.apply_external(&new);
        } else {
            // Rebuild what the file looked like before the agent's edit, so
            // the change bars and undo show exactly what the agent did.
            let mut before = new.clone();
            for (old, newer) in diffs.iter().rev() {
                match old {
                    None => before = String::new(),
                    Some(o) if !newer.is_empty() && before.contains(newer.as_str()) => {
                        before = before.replacen(newer.as_str(), o, 1);
                    }
                    _ => {}
                }
            }
            let mut doc = Document::from_text(&before, Some(path.clone()));
            doc.saved_text = before.replace("\r\n", "\n");
            let mut ed = Editor::new(doc);
            ed.apply_external(&new);
            self.editors.push(ed);
            if self.editors.len() == 1 {
                self.active = 0;
            }
        }
        self.toast(format!("✦ Agent edited {name}"), ToastKind::Success);
    }

    /// Send what is in the agent input box.
    pub fn agent_send(&mut self) {
        if self.agent.is_working() || self.automation.busy() {
            self.toast(
                "Wait for the current turn or stop it first",
                ToastKind::Info,
            );
            return;
        }
        let text = std::mem::take(&mut self.agent.input);
        self.agent.cursor = 0;
        if text.trim().is_empty() {
            return;
        }
        if text.len() > 32 * 1024 {
            self.agent.cursor = text.len();
            self.agent.input = text;
            self.toast("Task text exceeds 32 KiB; split the goal or reference files so descendants receive the full goal",ToastKind::Error);
            return;
        }
        self.reload_skills();
        self.agent.skill_context =
            match crate::skills::required_context(&self.skills, &self.agent.options.skills) {
                Ok(context) => context,
                Err(e) => {
                    self.agent.cursor = text.len();
                    self.agent.input = text;
                    self.toast(format!("Cannot start agent: {e:#}"), ToastKind::Error);
                    return;
                }
            };
        self.setup_bridge();
        // The agent reads files from disk: save your work first.
        let mut saved = 0;
        for e in &mut self.editors {
            if e.doc.is_dirty() && e.doc.path.is_some() && e.doc.save().is_ok() {
                saved += 1;
            }
        }
        if saved > 0 {
            self.toast(
                format!("Saved {saved} file(s) so the agent sees them"),
                ToastKind::Info,
            );
        }
        let context = if self.agent.options.auto_context {
            self.agent_context()
        } else {
            Vec::new()
        };
        self.begin_automation_turn(&text, serde_json::json!({"editor_context":context}));
        self.agent.prompt(&text, context);
    }

    /// The open file (and selection) as ACP context blocks.
    fn agent_context(&self) -> Vec<serde_json::Value> {
        let Some(e) = self.editor() else {
            return Vec::new();
        };
        let Some(path) = e.doc.path.as_ref() else {
            return Vec::new();
        };
        if crate::credentials::is_private_path(path) {
            return Vec::new();
        }
        let uri = agent::file_uri(path);
        let mut blocks = vec![serde_json::json!({
            "type": "resource_link", "uri": uri, "name": e.doc.name()
        })];
        if !e.sel.is_empty() {
            let (sl, _) = e.doc.pos(e.sel.start());
            let (el, _) = e.doc.pos(e.sel.end());
            let text = e.selected_text();
            if self.agent.embedded_context {
                blocks.push(serde_json::json!({
                    "type": "resource",
                    "resource": { "uri": format!("{uri}#L{}-{}", sl + 1, el + 1), "mimeType": "text/plain", "text": text }
                }));
            } else {
                blocks.push(serde_json::json!({
                    "type": "text",
                    "text": format!("Selected lines {}-{} of {}:\n```\n{text}\n```", sl + 1, el + 1, e.doc.name())
                }));
            }
        }
        blocks
    }

    /// Short description of the context that will be attached (for the chip).
    pub fn agent_context_label(&self) -> Option<String> {
        if !self.agent.options.auto_context {
            return None;
        }
        let e = self.editor()?;
        let path = e.doc.path.as_ref()?;
        if crate::credentials::is_private_path(path) {
            return None;
        }
        let name = e.doc.name();
        if e.sel.is_empty() {
            Some(name)
        } else {
            let (sl, _) = e.doc.pos(e.sel.start());
            let (el, _) = e.doc.pos(e.sel.end());
            Some(format!("{name}:{}-{}", sl + 1, el + 1))
        }
    }

    fn agent_target(&mut self, t: AgentTarget) {
        match t {
            AgentTarget::OpenFile(p, line) => {
                self.open_file(&p);
                if let (Some(l), Some(e)) = (line, self.editor_mut()) {
                    e.goto_line(l.saturating_sub(1) as usize);
                }
                if self.compact {
                    self.show_agent = false;
                }
            }
            AgentTarget::Permission(i, opt) => self.agent.choose(i, Some(&opt)),
            AgentTarget::Retry => {
                self.agent.available = agent::detect_agents();
                if self.agent.spec.is_none() {
                    self.agent.spec = self.agent.available.first().cloned();
                }
                self.cancel_automation();
                self.agent.stop();
                self.setup_bridge();
                self.agent.start();
            }
            AgentTarget::NewChat => self.run(Command::AgentNewChat),
            AgentTarget::Close => {
                self.show_agent = false;
                self.focus = if self.editors.is_empty() {
                    Focus::Explorer
                } else {
                    Focus::Editor
                };
            }
            AgentTarget::Model => self.run(Command::AgentModel),
            AgentTarget::PickAgent => self.run(Command::AgentPick),
            AgentTarget::Settings => self.run(Command::AgentSettings),
            AgentTarget::Stop => self.run(Command::AgentStop),
            AgentTarget::Send => {
                self.focus = Focus::Agent;
                self.agent_send();
            }
            AgentTarget::Suggest(text) => {
                self.focus = Focus::Agent;
                self.agent.input = text;
                self.agent_send();
            }
        }
    }

    fn agent_key(&mut self, code: KeyCode, ctrl: bool, shift: bool, alt: bool) {
        if code == KeyCode::Esc && (self.agent.is_working() || self.automation.busy()) {
            self.run(Command::AgentStop);
            return;
        }
        // Number keys answer a pending permission request.
        if self.agent.input.is_empty()
            && let KeyCode::Char(c @ '1'..='9') = code
            && let Some(i) = self.agent.pending_permission()
        {
            let n = c as usize - '1' as usize;
            let opt = match &self.agent.entries[i] {
                agent::Entry::Permission(p) => p.options.get(n).map(|o| o.0.clone()),
                _ => None,
            };
            if let Some(o) = opt {
                self.agent.choose(i, Some(&o));
            }
            return;
        }
        let a = &mut self.agent;
        match code {
            KeyCode::Esc => {
                if a.is_working() {
                    a.cancel();
                } else {
                    self.focus = if self.editors.is_empty() {
                        Focus::Explorer
                    } else {
                        Focus::Editor
                    };
                    if self.compact {
                        self.show_agent = false;
                    }
                }
            }
            KeyCode::Enter if shift || alt => {
                a.input.insert(byte_at(&a.input, a.cursor), '\n');
                a.cursor += 1;
            }
            KeyCode::Enter => self.agent_send(),
            KeyCode::Tab | KeyCode::BackTab => {
                if !self.editors.is_empty() {
                    self.focus = Focus::Editor;
                }
            }
            KeyCode::Backspace if ctrl || alt => {
                let before: String = a.input.chars().take(a.cursor).collect();
                let trimmed = before.trim_end();
                let cut = trimmed
                    .rfind(char::is_whitespace)
                    .map(|b| trimmed[..b + 1].chars().count())
                    .unwrap_or(0);
                let rest: String = a.input.chars().skip(a.cursor).collect();
                a.input = format!("{}{rest}", before.chars().take(cut).collect::<String>());
                a.cursor = cut;
            }
            KeyCode::Backspace => {
                if a.cursor > 0 {
                    let b = byte_at(&a.input, a.cursor - 1);
                    a.input.remove(b);
                    a.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                if a.cursor < a.input.chars().count() {
                    let b = byte_at(&a.input, a.cursor);
                    a.input.remove(b);
                }
            }
            KeyCode::Left => a.cursor = a.cursor.saturating_sub(1),
            KeyCode::Right => a.cursor = (a.cursor + 1).min(a.input.chars().count()),
            KeyCode::Home => a.cursor = 0,
            KeyCode::End => a.cursor = a.input.chars().count(),
            KeyCode::Up => a.scroll += 1,
            KeyCode::Down => a.scroll = a.scroll.saturating_sub(1),
            KeyCode::PageUp => a.scroll += 10,
            KeyCode::PageDown => a.scroll = a.scroll.saturating_sub(10),
            KeyCode::Char(c) if !ctrl && !alt => {
                a.input.insert(byte_at(&a.input, a.cursor), c);
                a.cursor += 1;
            }
            _ => {}
        }
    }
}

fn byte_at(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

fn git_branch(root: &Path) -> Option<String> {
    let mut dir = Some(root);
    while let Some(d) = dir {
        let head = d.join(".git").join("HEAD");
        if let Ok(s) = std::fs::read_to_string(&head) {
            let s = s.trim();
            return Some(match s.strip_prefix("ref: refs/heads/") {
                Some(b) => b.to_string(),
                None => s.chars().take(8).collect(),
            });
        }
        dir = d.parent();
    }
    None
}
