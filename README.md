# termed

A terminal code editor that looks and feels like the Visual Studio C# editor, for any language.

![termed editing C# with the Visual Studio Dark theme](docs/editor.png)

| Command palette | Find & replace (Termed Neon theme) |
|---|---|
| ![Command palette](docs/command-palette.png) | ![Find and replace](docs/find-neon.png) |

## AI agent (free, no setup)

![The agent panel fixing a bug with OpenCode's free model](docs/agent.png)

Press `Ctrl+K A`, click **✦ Agent** in the title bar, or tap **✦** on a phone. Then ask in plain words, or tap one of the suggestions ("Find bugs in calc.py"). The agent can read and edit your project:

- **Free by default, nothing to install.** termed has a built-in agent, **termed AI**, written in Rust. It uses the free models of [Kilo Gateway](https://kilo.ai/docs/gateway/authentication), which allows anonymous use of its `:free` models with no account and no API key. Because it needs nothing else installed, it also works in **Termux on Android**. It can list, read, search, edit and create files, and run commands; every command asks for your approval first.
- **OpenCode, if you have it.** On desktop, if [OpenCode](https://opencode.ai) is installed, termed uses it by default through the [Agent Client Protocol](https://agentclientprotocol.com); its free models also need no account. You can switch between agents with *Agent: Choose Agent*. On Android, OpenCode's npm package doesn't install, so termed AI is the default there.
- **Free models are busy sometimes.** Free models are shared and rate-limited (Kilo allows 200 requests per hour per IP address, and one reply can take several requests). If a model reports it is busy, or stays silent for 25 seconds, termed switches to another free model and asks again automatically, telling you in the chat. Tap the model name to choose one yourself; free models are listed first. Set `TERMED_FREE_MODEL` to change the built-in agent's starting model.
- **Privacy.** Your prompt, the open file and the files the agent reads are sent to the model's provider (with Ollama they stay on your device). Free models may log or train on what they receive, so don't use them with secrets or confidential code.
- **You stay in control.** The agent's edits appear in your open files as normal edits with change bars, and one Ctrl+Z (↶) reverts an edit. Diffs show inline in the chat, and tapping a change opens the file at that spot. If the agent asks permission to run a tool, you tap Allow or Reject (or press 1–3).
- **Context is automatic.** The open file, and your selection if there is one, are attached to every message (shown as `@ file.py:12-20` above the prompt). Unsaved files are saved first so the agent sees your latest code.
- **Esc or ■ stops the agent.** + New starts a fresh chat.
- **More providers, same agent.** *Agent: Choose Agent* lists termed AI on other model providers too. All of them are plain HTTPS from Rust, so they work on desktop and in Termux alike:

  | Provider | Cost | Setup |
  |---|---|---|
  | Kilo Gateway (default) | free | none |
  | [Ollama](https://ollama.com) | free, runs on your device | install Ollama (`pkg install ollama` on Termux), `ollama pull qwen2.5-coder`; `OLLAMA_HOST` is honored |
  | [OpenRouter](https://openrouter.ai/keys) | free models with a free key (`openrouter/free`) | API key |
  | [Google Gemini](https://aistudio.google.com/apikey), [Groq](https://console.groq.com/keys), [Cerebras](https://cloud.cerebras.ai), [Mistral](https://console.mistral.ai/api-keys) | free tiers | API key |
  | [Anthropic Claude](https://platform.claude.com/settings/keys) | paid | API key; uses Claude Opus 5.5 through Anthropic's native Messages API, with adaptive thinking and server-side refusal fallbacks turned on |
  | [OpenAI](https://platform.openai.com/api-keys), [DeepSeek](https://platform.deepseek.com/api_keys), [xAI](https://console.x.ai) | paid | API key |
  | Any OpenAI-compatible server (LM Studio, llama.cpp, vLLM, LiteLLM…) | — | `TERMED_OPENAI_BASE_URL` (+ `TERMED_OPENAI_API_KEY`) |

  Picking a provider that needs a key asks for it once (the field is masked). The key is saved to `~/.config/termed/config.json`, readable only by you; the usual environment variables (`ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, `OPENROUTER_API_KEY`, …) work too and take precedence. *Agent: Set API Key* changes it later. termed remembers the agent you picked last. Paid usage shows token counts only, never "free". Set `TERMED_MODEL` to choose the starting model of a keyed provider.
- **Other agents work too.** termed detects about 20 [ACP](https://agentclientprotocol.com) agents when installed: OpenCode, Gemini CLI, Qwen Code, Claude Code, Codex, GitHub Copilot, Cline, Kilo Code, Goose, Cursor, Auggie, Grok, Factory Droid, Kimi, Devin, Mistral Vibe, Junie, Qoder, Stakpak and VT Code. If Node.js is installed, Gemini CLI, Qwen Code, Claude Code, Codex and GitHub Copilot can also start straight from npm through `npx` without installing them first (on Termux only the pure-JavaScript ones, Gemini CLI and Qwen Code, are offered). When an agent needs you to sign in, the panel shows its sign-in options as buttons. Keys you saved in termed are passed on to them (for example a Gemini key as `GEMINI_API_KEY`). Any other ACP agent works with `TERMED_AGENT="command args"`; `TERMED_AGENT=builtin` (or `builtin:ollama`, `builtin:anthropic`, …) forces the built-in agent on a provider.

  ![Choosing an agent or provider](docs/agent-picker.png)
- **On a phone** the panel is a full-screen sheet above the touch keyboard ([screenshot](docs/agent-phone.png)).

## On a phone

![termed on a 46-column phone screen: explorer drawer, wrapped C#, find, command palette](docs/mobile.png)

termed adapts to phone terminals such as Termux, Blink, Termius and iSH. Below 80 columns it switches to a compact layout automatically; `--mobile` (or `TERMED_MOBILE=1`) forces it at any size. Rotating the device or resizing the window re-lays out the screen immediately.

- **Word wrap.** Long lines wrap, so you never scroll sideways. Wrapped rows keep the line's indent and get a `↪` marker in the gutter, and long expressions break at spaces, commas, brackets or before a `.`. `Alt+Z` or the **Wrap** button toggles it.
- **Touch keyboard.** A fixed 2×9 grid of the keys a phone keyboard lacks, laid out like a laptop: modifiers on the left and an arrow cluster with ↑ above ↓ and ← → beside it. Every key always sits in the same place, so you can build muscle memory. **Fn** switches to an actions page (Copy, Cut, Paste, Save, Find, Replace, go to line, all commands, select all/line/word, comment, duplicate, move line, wrap). The keyboard stays on in Termux, Blink and a-Shell even in landscape; `--no-toolbar` hides it.

  ![Touch keyboard states: one-shot Ctrl, locked Shift selecting, box selection, Fn page](docs/touch-keys.png)

- **Sticky Ctrl / Alt / Shift.** A soft keyboard can't hold a modifier while you press another key, so the toolbar modifiers latch:
  - **Tap once** and the key is tinted and underlined, and the status bar shows `CTRL+`. It applies to the **next key only**, whether you tap a toolbar key, type on the soft keyboard or tap the text.
  - **Tap twice** and it is **locked** (solid) until you tap it again or press Esc.
  - Modifiers combine. For example, Ctrl+Shift+→ selects a word, Alt+↑ moves the line up, Ctrl+`s` (typed on the soft keyboard) saves, and Ctrl locked lets you press → → → to jump word by word.
  - **Locked Shift is selection mode** (status `SELECT`). Arrows, Home/End and taps extend the selection. Typing replaces the selection and ends the mode, and Copy or Cut also ends it.
- **Box (column) selection.** Tap **Box**, which locks Alt+Shift, then use the arrows or tap a corner to draw a rectangle. The status bar shows `BOX 4×6`, and the box may extend past the end of short lines. Typing, Tab, Backspace and Delete apply to **every line** at once; short lines are padded with spaces. Copy and cut take the column, and pasting a column puts one line on each row. Tap **Box** or Esc to leave. On a hardware keyboard, use Visual Studio's `Alt+Shift+arrows` or Alt+drag with the mouse.
- **Selection by touch.** Double-tap selects a word and triple-tap a line. Shift+tap (Shift latched, then tap) extends the selection to the tapped point.
- **Explorer drawer.** ☰ slides the file tree over the editor. Tapping a file opens it and closes the drawer, and tapping outside dismisses it.
- **Compact chrome.** The title bar has large ☰, ⌕ (open file) and ⋯ (commands) buttons, and tapping the file name opens quick open. The tab row appears only when more than one file is open. Breadcrumbs, the minimap and keyboard hints are hidden. The status bar drops lower-priority items first.
- **Phone-sized overlays.** The command palette is edge to edge, the find/replace bar spans the full width, and dialog text wraps; dialog buttons stack when they don't fit side by side. The welcome screen is a list of large buttons.
- **Gestures and keyboards.** Swipes scroll, a tap moves the cursor (including on wrapped rows), and you can drag the scrollbar. Keyboards that send LF for Return work as Enter.

## High-resolution graphics

A terminal is a grid of character cells, but termed draws finer than the grid wherever it can:

- **Real pixels.** At startup termed asks the terminal whether it supports a graphics protocol: the [kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/) (kitty, Ghostty), [Sixel](https://en.wikipedia.org/wiki/Sixel) (foot, Windows Terminal, mlterm, xterm with Sixel enabled, and others) or iTerm2 inline images (iTerm2, WezTerm). If it does, the welcome logo and pictures are drawn as real anti-aliased images with a soft glow. It also asks for the cell size in pixels so images fit exactly.
- **Pictures open in tabs.** PNG, JPEG, GIF, WebP, BMP and ICO files open as view-only image tabs, centered and fitted to the editor. A checkerboard shows through transparent parts. The tab can never write to the file.
- **Sub-cell mosaics everywhere else.** Without a graphics protocol, pictures are drawn with block-mosaic glyphs that split each cell into 2×4 octants (Unicode 16), 2×3 sextants, 2×2 quadrants or half blocks. For every cell, termed picks the two colors and the glyph shape that best match the sub-pixels underneath. termed uses octants where the terminal draws them itself (kitty, Ghostty, foot), sextants on WezTerm and Windows Terminal, and quadrants elsewhere, because every common monospace font has them. If the grid is too coarse for a legible logo, the hand-drawn text logo is used instead.
- **Smooth scrollbar.** The scrollbar thumb moves in eighths of a cell (using ▁▂▃▄▅▆▇), so it glides instead of jumping a whole row.
- **Soft shadows.** Popups cast a two-step shadow that fades out.
- **No tearing.** Every frame is sent as one synchronized update (DEC mode 2026), so the terminal never shows half-drawn frames. Terminals without it simply ignore it.

| Terminal with pixel graphics | Octant mosaic fallback |
|---|---|
| ![Welcome screen with a real-pixel logo](docs/welcome.png) | ![Welcome screen with an octant mosaic logo](docs/welcome-octants.png) |
| ![A PNG open in an image tab](docs/image-view.png) | ![The same PNG as an octant mosaic](docs/image-view-octants.png) |

*View: Graphics Info* shows what termed detected. `TERMED_GRAPHICS=kitty|sixel|iterm2|off` overrides the pixel protocol, and `TERMED_GLYPHS=octant|sextant|quadrant|half` overrides the mosaic.

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
- Box (column) selection like Visual Studio: Alt+Shift+arrows or Alt+drag. Typing, Tab, Backspace and Delete edit every line of the column, and copy/paste work per line.
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
termed --mobile src/main.rs # force the phone layout
```

On Android, install [Termux](https://termux.dev), run `pkg install rust git clang`, then `cargo install --path .`. The AI agent works there out of the box.

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
| Duplicate line(s) | `Ctrl+Shift+D` |
| Box (column) selection | `Alt+Shift+arrows`, Alt+drag |
| Delete line | `Ctrl+Shift+K` |
| Insert line below / above | `Ctrl+Enter` / `Ctrl+Shift+Enter` |
| Indent / outdent | `Tab` / `Shift+Tab`, `Ctrl+]` / `Ctrl+[` |
| Delete word left / right | `Ctrl+Backspace` or `Alt+Backspace` / `Ctrl+Delete` |
| Color theme picker / next theme | `Ctrl+K T` / `F8` |
| Language mode | `Ctrl+K Ctrl+M` |
| Toggle minimap / whitespace / indent guides / line numbers | `Ctrl+K M` / `W` / `I` / `L` |
| Toggle word wrap | `Alt+Z` |
| AI agent panel | `Ctrl+K A` |

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
| Pixel graphics | [ratatui-image](https://github.com/ratatui/ratatui-image) + [image](https://github.com/image-rs/image) | Detects kitty, Sixel and iTerm2 support and the cell size, and encodes pictures for them (Sixel through the pure-Rust `icy_sixel`). The block-mosaic fallback with its two-color fitting is termed's own (`ui/hires.rs`). |
| AI agents | [Agent Client Protocol](https://agentclientprotocol.com) + [serde_json](https://github.com/serde-rs/json) + [ureq](https://github.com/algesten/ureq) | ACP is the open standard Zed and JetBrains use for coding agents. ureq (with rustls and bundled root certificates) lets the built-in agent reach every provider from anywhere, including Android. |
| Text width | [unicode-width](https://github.com/unicode-rs/unicode-width) | Correct columns for CJK and wide characters. |

Source layout:

```
src/
  document.rs     rope buffer, undo/redo transactions, line endings, indent detection
  editor.rs       caret/selection model, soft wrap and VS-style editing commands
  highlight.rs    incremental, time-budgeted highlighting + string-aware bracket analysis
  theme.rs        Visual Studio Dark / Termed Neon + derived palettes for bundled themes
  graphics.rs     pixel graphics: protocol detection, image placement, mosaic cache
  agent.rs        Agent Client Protocol client: launches the agent, JSON-RPC, chat model, free-model failover
  builtin_agent.rs  built-in agent: OpenAI-compatible and Anthropic Messages streaming, file/search/command tools
  providers.rs    model providers, API key store (~/.config/termed)
  app.rs          state, key/mouse handling, palette, find/replace, dialogs
  ui/             title bar, explorer/drawer, tabs, code view, minimap, toolbar, overlays,
                  hires.rs: sub-cell canvas (half/quadrant/sextant/octant) with two-color fitting
examples/
  snapshot.rs     renders frames to HTML headlessly (used for the screenshots)
```

## Development

```sh
cargo test                  # includes phone layout/touch tests and agent tests against a scripted ACP agent
cargo clippy --all-targets
# Render a frame to HTML, replaying keystrokes, taps and swipes:
cargo run --example snapshot -- out.html 150x42 "Visual Studio Dark" . src/main.rs -- ctrl+p text:editor
cargo run --example snapshot -- phone.html 46x30 "Visual Studio Dark" . src/main.rs -- tap:1,0 swipeup:20,10 resize:100x24
```
