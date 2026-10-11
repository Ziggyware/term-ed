//! Every user-facing action, shared by key bindings and the command palette.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    QuickOpen,
    CommandPalette,
    NewFile,
    Save,
    SaveAs,
    SaveAll,
    CloseTab,
    NextTab,
    PrevTab,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    SelectLine,
    SelectNextOccurrence,
    Find,
    Replace,
    FindNext,
    FindPrev,
    GotoLine,
    GotoBracket,
    ToggleComment,
    Indent,
    Outdent,
    MoveLineUp,
    MoveLineDown,
    DuplicateLineDown,
    DuplicateLineUp,
    DeleteLine,
    InsertLineBelow,
    InsertLineAbove,
    ToggleSidebar,
    FocusExplorer,
    ToggleMinimap,
    ToggleWhitespace,
    ToggleIndentGuides,
    ToggleLineNumbers,
    SelectTheme,
    NextTheme,
    SelectLanguage,
    ConvertIndentToSpaces,
    ConvertIndentToTabs,
    ToggleLineEnding,
    RefreshExplorer,
    RevealInExplorer,
    ShowKeybindings,
    ToggleWordWrap,
    ToggleToolbar,
    ToggleSelectMode,
}

pub struct CommandInfo {
    pub cmd: Command,
    pub title: &'static str,
    pub keys: &'static str,
}

macro_rules! cmds {
    ($($cmd:ident, $title:expr, $keys:expr;)*) => {
        pub const COMMANDS: &[CommandInfo] = &[$(CommandInfo { cmd: Command::$cmd, title: $title, keys: $keys },)*];
    };
}

cmds! {
    QuickOpen, "Go to File...", "Ctrl+P";
    CommandPalette, "Show All Commands", "Ctrl+Shift+P / F1";
    NewFile, "File: New File", "Ctrl+N";
    Save, "File: Save", "Ctrl+S";
    SaveAs, "File: Save As...", "Ctrl+Alt+S";
    SaveAll, "File: Save All", "Ctrl+K S";
    CloseTab, "View: Close Editor", "Ctrl+W";
    NextTab, "View: Next Editor", "Ctrl+PgDn";
    PrevTab, "View: Previous Editor", "Ctrl+PgUp";
    Quit, "File: Exit", "Ctrl+Q";
    Undo, "Edit: Undo", "Ctrl+Z";
    Redo, "Edit: Redo", "Ctrl+Y";
    Cut, "Edit: Cut", "Ctrl+X";
    Copy, "Edit: Copy", "Ctrl+C";
    Paste, "Edit: Paste", "Ctrl+V";
    SelectAll, "Selection: Select All", "Ctrl+A";
    SelectLine, "Selection: Expand Line Selection", "Ctrl+L";
    SelectNextOccurrence, "Selection: Add Next Occurrence", "Ctrl+D";
    Find, "Edit: Find", "Ctrl+F";
    Replace, "Edit: Replace", "Ctrl+H";
    FindNext, "Edit: Find Next", "F3";
    FindPrev, "Edit: Find Previous", "Shift+F3";
    GotoLine, "Go to Line...", "Ctrl+G";
    GotoBracket, "Go to Bracket", "Ctrl+\\";
    ToggleComment, "Edit: Toggle Line Comment", "Ctrl+/";
    Indent, "Edit: Indent Line", "Tab";
    Outdent, "Edit: Outdent Line", "Shift+Tab";
    MoveLineUp, "Edit: Move Line Up", "Alt+Up";
    MoveLineDown, "Edit: Move Line Down", "Alt+Down";
    DuplicateLineDown, "Edit: Duplicate Line", "Ctrl+Shift+D";
    DuplicateLineUp, "Edit: Duplicate Line Up", "";
    DeleteLine, "Edit: Delete Line", "Ctrl+Shift+K";
    InsertLineBelow, "Edit: Insert Line Below", "Ctrl+Enter";
    InsertLineAbove, "Edit: Insert Line Above", "Ctrl+Shift+Enter";
    ToggleSidebar, "View: Toggle Explorer", "Ctrl+B";
    FocusExplorer, "View: Focus Explorer", "Ctrl+E";
    ToggleMinimap, "View: Toggle Minimap", "Ctrl+K M";
    ToggleWhitespace, "View: Toggle Render Whitespace", "Ctrl+K W";
    ToggleIndentGuides, "View: Toggle Indent Guides", "Ctrl+K I";
    ToggleLineNumbers, "View: Toggle Line Numbers", "Ctrl+K L";
    SelectTheme, "Preferences: Color Theme", "Ctrl+K T";
    NextTheme, "Preferences: Next Color Theme", "F8";
    SelectLanguage, "Change Language Mode", "Ctrl+K Ctrl+M";
    ConvertIndentToSpaces, "Convert Indentation to Spaces", "";
    ConvertIndentToTabs, "Convert Indentation to Tabs", "";
    ToggleLineEnding, "Change End of Line Sequence (LF/CRLF)", "";
    RefreshExplorer, "Explorer: Refresh", "";
    RevealInExplorer, "Explorer: Reveal Active File", "";
    ShowKeybindings, "Help: Keyboard Shortcuts", "Ctrl+K Ctrl+S";
    ToggleWordWrap, "View: Toggle Word Wrap", "Alt+Z";
    ToggleToolbar, "View: Toggle Touch Toolbar", "";
    ToggleSelectMode, "Selection: Toggle Selection Mode (lock Shift)", "";
}
