//! Application state and input handling.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use regex::{Regex, RegexBuilder};

use crate::agent::{self, Agent, AgentEvent, State as AgentState};
use crate::clipboard::Clipboard;
use crate::commands::{COMMANDS, Command};
use crate::document::{Document, Indent, LineEnding, Selection};
use crate::editor::Editor;
use crate::explorer::{self, Explorer};
use crate::fuzzy;
use crate::highlight;
use crate::theme::{self, ThemeEntry};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Explorer,
    Find,
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
}

pub struct Palette {
    pub mode: PaletteMode,
    pub input: String,
    pub cursor: usize,
    pub items: Vec<String>,
    pub hints: Vec<String>,
    pub filtered: Vec<fuzzy::Scored>,
    pub selected: usize,
    pub scroll: usize,
    /// Theme active before a live-preview picker opened (restored on Esc).
    pub restore_theme: Option<String>,
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
    Install,
    Retry,
    NewChat,
    Close,
    Model,
    PickAgent,
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
    pub toasts: Vec<Toast>,
    pub settings: Settings,
    pub git_branch: Option<String>,
    pub layout: Layout,
    pub quit: bool,
    /// Phone-sized screen: single-column layout (set by the renderer each frame).
    pub compact: bool,
    /// Latched modifiers from the touch toolbar.
    pub mods: StickyMods,
    pub agent: Agent,
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
            toasts: Vec::new(),
            settings: Settings {
                minimap: true,
                whitespace: false,
                indent_guides: true,
                line_numbers: true,
                nerd_font,
                word_wrap: None,
                toolbar: None,
                force_compact: false,
            },
            compact: false,
            mods: StickyMods::default(),
            agent: Agent::new(root.clone()),
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
        for f in files {
            app.open_file(&f);
        }
        if app.editors.is_empty() {
            app.focus = Focus::Explorer;
        }
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
        let before = self.toasts.len();
        self.toasts
            .retain(|t| t.at.elapsed() < Duration::from_millis(3500));
        before != self.toasts.len()
    }

    pub fn open_file(&mut self, path: &Path) {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(i) = self
            .editors
            .iter()
            .position(|e| e.doc.path.as_deref() == Some(&path))
        {
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
                        format!("Cannot open {}: {err}", path.display()),
                        ToastKind::Error,
                    );
                    return;
                }
            }
        }
        self.focus = Focus::Editor;
        self.explorer.reveal(&path);
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
            p.input.insert_str(byte_at(&p.input, p.cursor), &t);
            p.cursor += t.chars().count();
            self.refilter();
        } else if self.focus == Focus::Find {
            let t = text.replace(['\n', '\r'], "");
            let (field, cur) = self.find_field();
            field.insert_str(byte_at(field, *cur), &t);
            *cur += t.chars().count();
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
            (KeyCode::Char('p'), true, false, _) | (KeyCode::Char('o'), true, ..) => {
                Some(Command::QuickOpen)
            }
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
            Focus::Agent => self.agent_key(code, ctrl, shift, alt),
            Focus::Explorer => self.explorer_key(code),
            Focus::Editor => self.editor_key(code, ctrl, shift, alt),
        }
    }

    fn editor_key(&mut self, code: KeyCode, ctrl: bool, shift: bool, alt: bool) {
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
                if find_visible {
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
    }

    fn activate_explorer_entry(&mut self) {
        if let Some(e) = self.explorer.selected_entry().cloned() {
            if e.is_dir {
                self.explorer.toggle(self.explorer.selected);
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
                for e in &mut self.editors {
                    if e.doc.is_dirty() && e.doc.path.is_some() && e.doc.save().is_ok() {
                        n += 1;
                    }
                }
                self.toast(format!("Saved {n} file(s)"), ToastKind::Success);
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
                self.agent.new_chat();
                self.show_agent = true;
                self.focus = Focus::Agent;
            }
            AgentStop => self.agent.cancel(),
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
            Find | Replace => {
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
                self.toast(
                    format!("Saved {name}  ·  {lines} lines"),
                    ToastKind::Success,
                );
                self.project_files = None;
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
            PendingClose::Quit => self.quit = true,
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
        let (items, hints): (Vec<String>, Vec<String>) = match mode {
            PaletteMode::Commands => COMMANDS
                .iter()
                .map(|c| (c.title.to_string(), c.keys.to_string()))
                .unzip(),
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
                    let hint = if a.free {
                        "free, no account"
                    } else {
                        "may need sign-in"
                    };
                    (a.name.clone(), hint.to_string())
                })
                .unzip(),
            PaletteMode::GotoLine | PaletteMode::SaveAs => (Vec::new(), Vec::new()),
        };
        let mut input = String::new();
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
            input,
            cursor,
            items,
            hints,
            filtered: Vec::new(),
            selected: 0,
            scroll: 0,
            restore_theme,
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
        match code {
            KeyCode::Esc => {
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
                    self.run(COMMANDS[s.index].cmd);
                }
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
                }
            }
            PaletteMode::AgentModels => {
                if let Some(v) = chosen {
                    self.agent.set_model(&v);
                }
                self.focus = Focus::Agent;
            }
            PaletteMode::AgentPicker => {
                if let Some(s) = p.filtered.get(p.selected) {
                    self.agent.use_agent(s.index);
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
                if let Some(e) = self.editors.get_mut(self.active) {
                    e.doc.path = Some(path.clone());
                    let first = e.doc.line_str(0);
                    e.hl.set_syntax(highlight::detect_syntax(Some(&path), &first));
                }
                self.save_active();
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

/// OpenCode's free models: Big Pickle and the "-free" variants.
pub fn is_free_model(value: &str) -> bool {
    value.contains("free") || value.ends_with("big-pickle")
}

impl App {
    // ================================================================ agent

    /// Drain agent messages and act on them. Returns true if anything changed.
    pub fn pump_agent(&mut self) -> bool {
        let (events, changed) = self.agent.poll();
        for ev in events {
            match ev {
                AgentEvent::FileChanged { path, diffs } => self.agent_file_changed(&path, &diffs),
                AgentEvent::ReadFile {
                    rpc_id,
                    path,
                    line,
                    limit,
                } => {
                    let text = match self
                        .editors
                        .iter()
                        .find(|e| e.doc.path.as_deref() == Some(path.as_path()))
                    {
                        Some(e) => Some(e.doc.rope.to_string()),
                        None => std::fs::read_to_string(&path).ok(),
                    };
                    match text {
                        Some(t) => self.agent.respond_read(rpc_id, &t, line, limit),
                        None => self
                            .agent
                            .respond_error(rpc_id, &format!("cannot read {}", path.display())),
                    }
                }
                AgentEvent::WriteFile {
                    rpc_id,
                    path,
                    content,
                } => {
                    let old = std::fs::read_to_string(&path).ok();
                    match std::fs::write(&path, &content) {
                        Ok(()) => {
                            self.agent.respond(rpc_id, serde_json::Value::Null);
                            let diffs = vec![(old, content)];
                            self.agent_file_changed(&path, &diffs);
                        }
                        Err(e) => self.agent.respond_error(rpc_id, &e.to_string()),
                    }
                }
                AgentEvent::TurnEnded => {
                    self.explorer.refresh();
                    self.project_files = None;
                }
            }
        }
        changed
    }

    /// Keep animating while the agent works (spinners, elapsed time).
    pub fn agent_animating(&self) -> bool {
        self.agent.is_working()
    }

    /// An agent changed `path` on disk: show it in the editor as an undoable
    /// change with change bars, so you can review it (and Ctrl+Z it).
    fn agent_file_changed(&mut self, path: &Path, diffs: &[(Option<String>, String)]) {
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
        let text = std::mem::take(&mut self.agent.input);
        self.agent.cursor = 0;
        if text.trim().is_empty() {
            return;
        }
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
        let context = self.agent_context();
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
        let e = self.editor()?;
        e.doc.path.as_ref()?;
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
            AgentTarget::Install => self.agent.install(),
            AgentTarget::Retry => {
                self.agent.available = agent::detect_agents();
                if self.agent.spec.is_none() {
                    self.agent.spec = self.agent.available.first().cloned();
                }
                self.agent.stop();
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
            AgentTarget::Stop => self.agent.cancel(),
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
