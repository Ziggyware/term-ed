# termed

A terminal code editor that looks and feels like the Visual Studio C# editor, for any language.

## Overview
termed is a lightweight terminal-based code editor with a familiar VS C# look and feel, featuring:
- Multiple language support with syntax highlighting and auto‑indentation.
- Powerful find/replace, fuzzy file search, and ZIP‑as‑folder navigation.
- Built‑in AI agent for natural‑language editing and commands.
- Cross‑platform: works on desktop terminals and phones (e.g., Termux, Blink).

## Install

**From crates.io**
```sh
cargo install termed
```

**From source**
```sh
cargo install --path .
```

On Android (Termux):
```sh
pkg install rust git clang
cargo install --path .
```

## Keybindings
| Action | Keys |
|---|---|
| Quick open | `Ctrl+P` |
| Command palette | `Ctrl+Shift+P`, `F1` |
| Save / save all | `Ctrl+S` / `Ctrl+K S` |
| New file / close tab / quit | `Ctrl+N` / `Ctrl+W` / `Ctrl+Q` |
| Next / previous tab | `Ctrl+PgDn` / `Ctrl+PgUp` |
| Toggle / focus explorer | `Ctrl+B` / `Ctrl+E` |
| Find / replace | `Ctrl+F` / `Ctrl+H` |
| Find in files | `Ctrl+Shift+I` |
| Plugin manager | `Ctrl+Alt+P` |
| Settings | `Ctrl+,` |
| AI agent panel | `Ctrl+K A` |

## Key Features
- **VS C# look & feel**: dark theme, bracket pairing, indent guides.
- **Extensive language support**: 192 languages, auto‑indent, smart block expansion.
- **Rich media**: inline image previews, ZIP folders, ZIP image previews.
- **Advanced search**: fuzzy match, regex, whole word, case‑sensitive.
- **Plugins & AI**: custom commands, built‑in AI agent, host tool integration.
- **Phone‑friendly layout**: adaptable to small terminals.

---

For more details, see the [docs](docs/).