//! termed binary: argument parsing and the terminal event loop.

use std::io::{Write, stdout};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::terminal::supports_keyboard_enhancement;
use ratatui::crossterm::{execute, queue};

use termed::app::App;
use termed::{highlight, theme, ui};

#[derive(Parser, Debug)]
#[command(
    name = "termed",
    version,
    about = "A terminal code editor with Visual Studio-grade visuals"
)]
struct Args {
    /// Files or a directory to open.
    paths: Vec<PathBuf>,
    /// Color theme (see --list-themes).
    #[arg(
        short,
        long,
        env = "TERMED_THEME",
        default_value = "Visual Studio Dark"
    )]
    theme: String,
    /// Use Nerd Font file icons.
    #[arg(long, env = "TERMED_NERD_FONT")]
    nerd_font: bool,
    /// Phone mode: compact layout, word wrap and the touch toolbar at any size.
    /// (These turn on automatically below 80 columns.)
    #[arg(long, env = "TERMED_MOBILE")]
    mobile: bool,
    /// Never show the touch toolbar.
    #[arg(long)]
    no_toolbar: bool,
    /// Print the available themes and exit.
    #[arg(long)]
    list_themes: bool,
    /// Print the supported languages and exit.
    #[arg(long)]
    list_languages: bool,
    /// Start the AG-UI streaming server & visual editor on the specified port.
    #[arg(long, env = "TERMED_AGUI_PORT")]
    agui_port: Option<u16>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    // `let _ =` on writes: piping into `head` must not panic on a closed pipe.
    if args.list_themes {
        let mut out = stdout().lock();
        for t in theme::theme_names() {
            let _ = writeln!(out, "{t}");
        }
        return Ok(());
    }
    if args.list_languages {
        let mut names: Vec<_> = highlight::syntax_set()
            .syntaxes()
            .iter()
            .filter(|s| !s.hidden)
            .map(|s| s.name.clone())
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        let mut out = stdout().lock();
        let _ = writeln!(out, "{} languages:", names.len());
        for n in names {
            let _ = writeln!(out, "  {n}");
        }
        return Ok(());
    }

    let mut root = std::env::current_dir()?;
    let mut files = Vec::new();
    for p in &args.paths {
        if p.is_dir() {
            root = std::path::absolute(p)?;
        } else {
            files.push(p.clone());
        }
    }
    if args.paths.len() == 1
        && files.len() == 1
        && let Some(parent) = std::path::absolute(&files[0])?.parent()
        && !root.starts_with(parent)
        && !parent.starts_with(&root)
    {
        root = parent.to_path_buf();
    }

    let truecolor = std::env::var("COLORTERM")
        .map(|v| v.contains("truecolor") || v.contains("24bit"))
        .unwrap_or(false)
        || std::env::var("TERM_PROGRAM")
            .is_ok_and(|t| matches!(t.as_str(), "iTerm.app" | "WezTerm" | "vscode" | "ghostty"))
        || std::env::var("TERMED_TRUECOLOR").is_ok();

    // Warm up the grammar set so the first frame is instant.
    let _ = highlight::syntax_set();
    let mut app = App::new(root, files, &args.theme, args.nerd_font);

    if let Some(port) = args.agui_port {
        let _ = termed::agui::spawn_agui_server(port, app.agent.agui.clone());
    }

    // Termux, iSH and similar run on touch screens even when the window is wide
    // (landscape, tablets, DeX): keep the toolbar there.
    let touch_terminal = std::env::var_os("TERMUX_VERSION").is_some()
        || std::env::var("PREFIX").is_ok_and(|p| p.contains("com.termux"))
        || std::env::var("TERM_PROGRAM").is_ok_and(|t| matches!(t.as_str(), "Blink" | "a-Shell"));
    if args.mobile {
        app.settings.force_compact = true;
        app.settings.toolbar = Some(true);
    } else if touch_terminal {
        app.settings.toolbar = Some(true);
    }
    if args.no_toolbar {
        app.settings.toolbar = Some(false);
    }

    let mut terminal = ratatui::init();
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    let mut out = stdout();
    execute!(
        out,
        EnableMouseCapture,
        EnableBracketedPaste,
        SetCursorStyle::BlinkingBar
    )?;
    if enhanced {
        // Lets us tell Ctrl+Shift+P from Ctrl+P, etc. (kitty keyboard protocol).
        queue!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }

    let result = run(&mut terminal, &mut app, truecolor);

    if enhanced {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        out,
        DisableMouseCapture,
        DisableBracketedPaste,
        SetCursorStyle::DefaultUserShape
    );
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App, truecolor: bool) -> Result<()> {
    let mut dirty = true;
    loop {
        if dirty {
            terminal.draw(|f| ui::draw(f, app, truecolor))?;
            dirty = false;
        }
        if app.quit {
            return Ok(());
        }
        let timeout = if app.busy() {
            Duration::from_millis(1)
        } else if app.agent.running() || app.agent_animating() {
            // Pick up streamed agent output promptly.
            Duration::from_millis(40)
        } else {
            Duration::from_millis(250)
        };
        if event::poll(timeout)? {
            app.handle_event(event::read()?);
            // Drain queued events (fast typing, mouse drags) before redrawing.
            while event::poll(Duration::ZERO)? {
                app.handle_event(event::read()?);
                if app.quit {
                    break;
                }
            }
            dirty = true;
        }
        if app.pump_agent() {
            dirty = true;
        }
        if app.tick() || app.busy() || app.agent_animating() {
            dirty = true;
        }
    }
}
