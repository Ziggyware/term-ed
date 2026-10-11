# termed

A terminal code editor that looks and feels like the Visual Studio C# editor, for any language.

![termed editing C# with the Visual Studio Dark theme](docs/editor.png)

| Command palette | Find & replace (Termed Neon theme) |
|---|---|
| ![Command palette](docs/command-palette.png) | ![Find and replace](docs/find-neon.png) |

## Features

**Visual Studio look**
- A *Visual Studio Dark* theme with the VS 2022 C# editor colors (blue keywords, teal types, purple control flow, salmon strings, green comments, teal line numbers). The colors are mapped to TextMate scopes, so every language gets the same look.
- Bracket-pair colorization (gold, orchid, blue) that ignores brackets inside strings and comments, plus matching-bracket highlight.
- Indent guides, with the guide for the current block highlighted.
- Change-tracking margin: green, blue and red bars mark lines added, modified or deleted since the last save.
- Highlight references: other occurrences of the identifier under the caret are tinted.
- A braille-dot **minimap** in syntax colors, and a scrollbar that doubles as an overview ruler showing find hits, changes and the caret.
- Current-line highlight, a thin bar caret, selections that extend past end of line, and whitespace rendering inside selections.
- Chrome: a title bar with a quick-open box, a Solution-Explorer style tree with file icons, tabs with dirty markers, breadcrumbs that show the enclosing class or function, and a clickable status bar.
- Floating panels with rounded borders and drop shadows: command palette, quick open, theme picker with live preview, language picker, go-to-line, a save dialog and toast notifications.
- 14 themes: Visual Studio Dark, Termed Neon (a phosphor-green hacker theme), Dracula, Catppuccin, One Half Dark, Nord, Gruvbox, Solarized, Monokai and more.

**Editing**
- Highlighting for 192 languages (C#, C/C++, Rust, Go, Python, TypeScript, Java, Kotlin, Swift, SQL, YAML, Markdown and more; run `termed --list-languages` for the full list).
- Auto-indent, smart block expansion when you press Enter between braces, and auto-closing brackets and quotes with type-over.
- Smart Home, word navigation, Ctrl+D to select the next occurrence, line move and duplicate, toggle comment for each language, and block indent and outdent.
- Undo and redo that group typing into words, like VS.
- Find and replace with match case, whole word and regex options (including `$1` capture groups), plus replace-one and replace-all.
- Copy with no selection copies the whole line, and pasting it inserts the line above, as in VS.
- Clipboard works through OSC 52 (over SSH and in tmux) and through `wl-copy`, `xclip`, `xsel` or `pbcopy`.
- Full mouse support: click, drag-select, double-click a word, triple-click a line, click the gutter to select a line, scroll, use tabs and the tree, drag the minimap or scrollbar, and resize the sidebar.
- Large files stay responsive. Highlighting is incremental and runs within a time budget, so jumping to the end of a 100k-line file is instant: provisional colors appear first and exact colors fill in over the next frames.

## Install

```sh
cargo install --path .
termed                      # open the current folder
termed src/Program.cs       # open a file
termed ~/code/project -t "Termed Neon"
```

You need a terminal with truecolor support to get the full look (Windows Terminal, iTerm2, WezTerm, kitty, Alacritty, GNOME Terminal and others). termed detects `COLORTERM=truecolor` and falls back to 256 colors when it isn't set; set `TERMED_TRUECOLOR=1` to force truecolor. Pass `--nerd-font` (or set `TERMED_NERD_FONT=1`) to use Nerd Font file icons.

## Keybindings

| Action | Keys |
|---|---|
| Quick open file | `Ctrl+P` |
| Command palette | `Ctrl+Shift+P`, `F1` |
| Save / save as / save all | `Ctrl+S` / `Ctrl+Alt+S` / `Ctrl+K S` |
| New file / close tab / quit | `Ctrl+N` / `Ctrl+W` / `Ctrl+Q` |
| Next / previous tab, tab *n* | `Ctrl+PgDn` / `Ctrl+PgUp`, `Alt+1…9` |
| Toggle / focus explorer | `Ctrl+B` / `Ctrl+E` |
| Find / replace / next / previous | `Ctrl+F` / `Ctrl+H` / `F3` / `Shift+F3` |
| Find options: case, word, regex | `Alt+C`, `Alt+W`, `Alt+R` |
| Go to line (`line:col`) | `Ctrl+G` |
| Go to matching bracket | `Ctrl+\` |
| Undo / redo | `Ctrl+Z` / `Ctrl+Y`, `Ctrl+Shift+Z` |
| Cut / copy / paste | `Ctrl+X` / `Ctrl+C` / `Ctrl+V` |
| Select all / line / next occurrence | `Ctrl+A` / `Ctrl+L` / `Ctrl+D` |
| Toggle comment | `Ctrl+/` |
| Move line(s) up / down | `Alt+Up` / `Alt+Down` |
| Copy line(s) up / down | `Alt+Shift+Up` / `Alt+Shift+Down` |
| Delete line | `Ctrl+Shift+K` |
| Insert line below / above | `Ctrl+Enter` / `Ctrl+Shift+Enter` |
| Indent / outdent | `Tab` / `Shift+Tab`, `Ctrl+]` / `Ctrl+[` |
| Delete word left / right | `Ctrl+Backspace` or `Alt+Backspace` / `Ctrl+Delete` |
| Color theme picker / next theme | `Ctrl+K T` / `F8` |
| Language mode | `Ctrl+K Ctrl+M` |
| Toggle minimap / whitespace / indent guides / line numbers | `Ctrl+K M` / `W` / `I` / `L` |

Some combinations (Ctrl+Shift+…, Ctrl+Enter, Ctrl+[) can only be told apart in terminals that support the kitty keyboard protocol, which termed turns on when it is available. In other terminals, Ctrl+Backspace arrives as Ctrl+H (Replace), so use Alt+Backspace to delete a word. Every command can also be run from the command palette.

## Architecture: the library behind each part

| Concern | Library | Why |
|---|---|---|
| Rendering | [ratatui](https://ratatui.rs) | Diffed cell-buffer rendering, so only changed cells are redrawn. The editor paints straight into the buffer to control every cell. |
| Terminal I/O | [crossterm](https://github.com/crossterm-rs/crossterm) | Cross-platform; supports mouse, bracketed paste, the kitty keyboard protocol and caret shape. |
| Text buffer | [ropey](https://github.com/cessen/ropey) | Rope with O(log n) edits and line indexing, the same structure Helix uses. |
| Syntax highlighting | [syntect](https://github.com/trishume/syntect) + [two-face](https://github.com/CosmicHorrorDev/two-face) | Sublime Text / TextMate grammars with bat's extended set of 192 languages, the same grammar format VS Code uses. |
| Fuzzy matching | [nucleo-matcher](https://github.com/helix-editor/nucleo) | The fzf-quality matcher from Helix, used for quick open and the palette. |
| File tree | [ignore](https://github.com/BurntSushi/ripgrep/tree/master/crates/ignore) | ripgrep's walker; respects `.gitignore`. |
| Change margin | [similar](https://github.com/mitsuhiko/similar) | Fast line diff against the saved file. |
| Find / replace | [regex](https://github.com/rust-lang/regex) | Linear-time regex with capture-group replacement. |
| Text width | [unicode-width](https://github.com/unicode-rs/unicode-width) | Correct columns for CJK and wide characters. |

Source layout:

```
src/
  document.rs     rope buffer, undo/redo transactions, line endings, indent detection
  editor.rs       caret/selection model and VS-style editing commands
  highlight.rs    incremental, time-budgeted highlighting + string-aware bracket analysis
  theme.rs        Visual Studio Dark / Termed Neon + derived palettes for bundled themes
  app.rs          state, key/mouse handling, palette, find/replace, dialogs
  ui/             title bar, explorer, tabs, breadcrumbs, code view, minimap, overlays
examples/
  snapshot.rs     renders frames to HTML headlessly (used for the screenshots)
```

## Development

```sh
cargo test
cargo clippy --all-targets
# Render a frame to HTML, replaying keystrokes:
cargo run --example snapshot -- out.html 150x42 "Visual Studio Dark" . src/main.rs -- ctrl+p text:editor
```
