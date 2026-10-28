//! The AI agent panel: conversation, tool activity with inline diffs, plans,
//! permission prompts and a prompt box. Docked on the right on wide screens,
//! a full-screen sheet on phones.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::util::{bold, ellipsize, fgbg, fill, put, width, wrap_text};
use crate::agent::{Entry, PlanStatus, State, ToolStatus};
use crate::app::{AgentTarget, App, Focus, is_free_model};
use crate::theme::{Palette, mix};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn spinner() -> &'static str {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    SPINNER[(ms / 90) as usize % SPINNER.len()]
}

/// One rendered row of the transcript.
#[derive(Default)]
struct Row {
    spans: Vec<(String, Style)>,
    bg: Option<Color>,
    target: Option<AgentTarget>,
    /// Extra click targets inside the row: (x offset, width, target).
    buttons: Vec<(u16, u16, AgentTarget)>,
}

impl Row {
    fn text(s: impl Into<String>, st: Style) -> Self {
        Row {
            spans: vec![(s.into(), st)],
            ..Default::default()
        }
    }
    fn blank() -> Self {
        Row::default()
    }
}

/// Split styled segments into words and greedily wrap them to `w` columns.
fn wrap_styled(segs: &[(String, Style)], w: usize, indent: &str) -> Vec<Vec<(String, Style)>> {
    let w = w.max(4);
    let mut tokens: Vec<(String, Style)> = Vec::new();
    for (s, st) in segs {
        let mut cur = String::new();
        for c in s.chars() {
            if c == ' ' {
                if !cur.is_empty() {
                    tokens.push((std::mem::take(&mut cur), *st));
                }
                tokens.push((" ".into(), *st));
            } else {
                cur.push(c);
            }
        }
        if !cur.is_empty() {
            tokens.push((cur, *st));
        }
    }
    let mut lines: Vec<Vec<(String, Style)>> = vec![Vec::new()];
    let mut used = 0usize;
    for (tok, st) in tokens {
        let tw = UnicodeWidthStr::width(tok.as_str());
        if tok == " " && used == 0 && lines.len() > 1 {
            continue;
        }
        if used + tw > w && used > 0 {
            lines.push(vec![(indent.to_string(), Style::default())]);
            used = UnicodeWidthStr::width(indent);
            if tok == " " {
                continue;
            }
        }
        if tw > w {
            // Hard-break very long words (URLs, paths).
            let mut chunk = String::new();
            let mut cw = 0;
            for c in tok.chars() {
                let ccw = c.width().unwrap_or(0);
                if used + cw + ccw > w {
                    lines
                        .last_mut()
                        .unwrap()
                        .push((std::mem::take(&mut chunk), st));
                    lines.push(Vec::new());
                    used = 0;
                    cw = 0;
                }
                chunk.push(c);
                cw += ccw;
            }
            lines.last_mut().unwrap().push((chunk, st));
            used += cw;
            continue;
        }
        lines.last_mut().unwrap().push((tok, st));
        used += tw;
    }
    lines
}

/// Hard-wrap a code line to `w` columns.
fn chunk(s: &str, w: usize) -> Vec<String> {
    let w = w.max(1);
    let mut out = vec![String::new()];
    let mut used = 0;
    for c in s.chars() {
        let c = if c == '\t' { ' ' } else { c };
        let cw = c.width().unwrap_or(0);
        if used + cw > w {
            out.push(String::new());
            used = 0;
        }
        out.last_mut().unwrap().push(c);
        used += cw;
    }
    out
}

/// Inline markdown: `code`, **bold**.
fn inline_md(line: &str, base: Style, code: Style) -> Vec<(String, Style)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_code = false;
    let mut in_bold = false;
    let mut chars = line.chars().peekable();
    let style = |c: bool, b: bool| {
        if c {
            code
        } else if b {
            base.add_modifier(Modifier::BOLD)
        } else {
            base
        }
    };
    while let Some(c) = chars.next() {
        if c == '`' {
            out.push((std::mem::take(&mut cur), style(in_code, in_bold)));
            in_code = !in_code;
        } else if c == '*' && chars.peek() == Some(&'*') && !in_code {
            chars.next();
            out.push((std::mem::take(&mut cur), style(in_code, in_bold)));
            in_bold = !in_bold;
        } else {
            cur.push(c);
        }
    }
    out.push((cur, style(in_code, in_bold)));
    out.retain(|(s, _)| !s.is_empty());
    out
}

fn markdown(text: &str, w: usize, pal: &Palette, panel_bg: Color, rows: &mut Vec<Row>) {
    let base = Style::default().fg(pal.fg);
    let code_st = Style::default().fg(pal.match_fg);
    let code_bg = mix(panel_bg, Color::Black, 0.35);
    let mut in_fence = false;
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            for c in chunk(line, w.saturating_sub(1)) {
                rows.push(Row {
                    spans: vec![(format!(" {c}"), Style::default().fg(pal.fg))],
                    bg: Some(code_bg),
                    ..Default::default()
                });
            }
            continue;
        }
        if line.is_empty() {
            rows.push(Row::blank());
            continue;
        }
        let trimmed = line.trim_start();
        let (prefix, body, st, indent) = if let Some(h) = trimmed.strip_prefix('#') {
            (
                "",
                h.trim_start_matches('#').trim(),
                bold(Style::default().fg(pal.accent)),
                "",
            )
        } else if let Some(b) = trimmed.strip_prefix("- ").or(trimmed.strip_prefix("* ")) {
            ("• ", b, base, "  ")
        } else {
            ("", line, base, "")
        };
        let mut segs = vec![(prefix.to_string(), Style::default().fg(pal.accent))];
        segs.extend(inline_md(body, st, code_st));
        for l in wrap_styled(&segs, w, indent) {
            rows.push(Row {
                spans: l,
                ..Default::default()
            });
        }
    }
}

fn tool_icon(kind: &str) -> &'static str {
    match kind {
        "read" => "◇",
        "edit" => "✎",
        "delete" => "✗",
        "move" => "⇄",
        "search" => "⌕",
        "execute" => "▶",
        "think" => "∴",
        "fetch" => "⇣",
        _ => "•",
    }
}

fn verb(kind: &str) -> &'static str {
    match kind {
        "read" => "Read",
        "edit" => "Edit",
        "delete" => "Delete",
        "move" => "Move",
        "search" => "Search",
        "execute" => "Run",
        "fetch" => "Fetch",
        "think" => "Think",
        _ => "Tool",
    }
}

/// Changed lines of a diff snippet, as (sign, text).
fn diff_lines(old: Option<&str>, new: &str) -> Vec<(char, String)> {
    let old = old.unwrap_or("");
    let d = similar::TextDiff::from_lines(old, new);
    let mut out = Vec::new();
    for change in d.iter_all_changes() {
        let sign = match change.tag() {
            similar::ChangeTag::Delete => '-',
            similar::ChangeTag::Insert => '+',
            similar::ChangeTag::Equal => continue,
        };
        out.push((sign, change.value().trim_end_matches('\n').to_string()));
    }
    out
}

fn build_rows(app: &App, w: usize, panel_bg: Color) -> Vec<Row> {
    let pal = &app.theme.palette;
    let a = &app.agent;
    let dim = Style::default().fg(pal.popup_dim);
    let mut rows: Vec<Row> = Vec::new();
    let name = a.name();

    match &a.state {
        State::NoAgent => {
            rows.push(Row::blank());
            rows.push(Row::text(
                "✦ No AI agent is selected",
                bold(Style::default().fg(pal.logo[0])),
            ));
            rows.push(Row::blank());
            for line in wrap_text(
                "Select the built-in agent in AI settings; no external agent installation is needed.",
                w,
            ) {
                rows.push(Row::text(line, dim));
            }
            rows.push(Row::blank());
            let label = " ⚙ AI settings ";
            rows.push(Row {
                spans: vec![(label.into(), bold(fgbg(pal.accent_fg, pal.accent)))],
                buttons: vec![(0, width(label), AgentTarget::Settings)],
                ..Default::default()
            });
            return rows;
        }
        State::Failed(msg) => {
            rows.push(Row::blank());
            for line in msg.lines() {
                for l in wrap_text(line, w) {
                    rows.push(Row::text(l, Style::default().fg(pal.error)));
                }
            }
            rows.push(Row::blank());
            let label = " ⟳ Retry ";
            rows.push(Row {
                spans: vec![(label.into(), bold(fgbg(pal.accent_fg, pal.accent)))],
                buttons: vec![(0, width(label), AgentTarget::Retry)],
                ..Default::default()
            });
            if !a.entries.is_empty() {
                rows.push(Row::blank());
            }
        }
        _ => {}
    }

    if a.entries.is_empty() && !matches!(a.state, State::Failed(_)) {
        rows.push(Row::blank());
        rows.push(Row::text(
            format!("✦ Ask {name} anything about your code."),
            bold(Style::default().fg(pal.fg)),
        ));
        rows.push(Row::blank());
        for l in wrap_text(
            "It can read and edit your project. Its edits show up in the editor with change bars, and Ctrl+Z (↶) undoes them.",
            w,
        ) {
            rows.push(Row::text(l, dim));
        }
        if a.spec.as_ref().is_some_and(|s| s.builtin) {
            rows.push(Row::blank());
            let privacy = if a.options.uses_router() {
                "NoKey Router uses your configured router key. Prompts and project content are sent to the model provider; avoid sharing secrets."
            } else {
                "Prompts and project content are sent to the configured model provider; avoid sharing secrets."
            };
            for l in wrap_text(privacy, w) {
                rows.push(Row::text(l, dim));
            }
        }
        rows.push(Row::blank());
        let file = app
            .editor()
            .and_then(|e| e.doc.path.as_ref().map(|_| e.doc.name()));
        let suggestions: Vec<String> = match file {
            Some(f) => vec![
                format!("Explain what {f} does"),
                format!("Find bugs in {f}"),
                format!("Write tests for {f}"),
                "Summarize this project".into(),
            ],
            None => vec![
                "Summarize this project".into(),
                "What should I work on next?".into(),
            ],
        };
        for s in suggestions {
            let label = format!(" › {s} ");
            let lw = width(&label).min(w as u16);
            rows.push(Row {
                spans: vec![(
                    ellipsize(&label, w),
                    fgbg(pal.fg, mix(panel_bg, pal.fg, 0.08)),
                )],
                buttons: vec![(0, lw, AgentTarget::Suggest(s))],
                ..Default::default()
            });
            rows.push(Row::blank());
        }
        if a.state == State::Starting {
            rows.push(Row::text(
                format!("{} Starting {name}…", spinner()),
                Style::default().fg(pal.accent),
            ));
        }
        return rows;
    }

    let last = a.entries.len().saturating_sub(1);
    for (i, e) in a.entries.iter().enumerate() {
        match e {
            Entry::User(t) => {
                rows.push(Row::blank());
                let tint = mix(panel_bg, pal.accent, 0.12);
                for line in t.lines() {
                    for l in wrap_text(line, w.saturating_sub(2)) {
                        rows.push(Row {
                            spans: vec![
                                ("▍ ".into(), Style::default().fg(pal.accent)),
                                (l, bold(Style::default().fg(pal.tab_active_fg))),
                            ],
                            bg: Some(tint),
                            ..Default::default()
                        });
                    }
                }
                rows.push(Row::blank());
            }
            Entry::Agent(t) => {
                // Models often emit bare newlines around tool calls: skip those.
                let t = t.trim_matches('\n');
                if t.trim().is_empty() {
                    continue;
                }
                if i > 0 && !matches!(a.entries[i - 1], Entry::User(_)) {
                    rows.push(Row::blank());
                }
                markdown(t, w, pal, panel_bg, &mut rows)
            }
            Entry::Thought(t) => {
                let st = Style::default()
                    .fg(pal.popup_dim)
                    .add_modifier(Modifier::ITALIC);
                let text = t.trim();
                if i == last && a.is_working() {
                    for l in wrap_text(&format!("∴ {text}"), w)
                        .into_iter()
                        .rev()
                        .take(3)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                    {
                        rows.push(Row::text(l, st));
                    }
                } else {
                    let first = text.lines().next().unwrap_or("");
                    rows.push(Row::text(ellipsize(&format!("∴ {first}"), w), st));
                }
            }
            Entry::Tool(t) => {
                let (mark, mark_st) = match t.status {
                    ToolStatus::Pending | ToolStatus::Running => {
                        (spinner(), Style::default().fg(pal.accent))
                    }
                    ToolStatus::Done => ("✓", Style::default().fg(pal.success)),
                    ToolStatus::Failed => ("✗", Style::default().fg(pal.error)),
                };
                let loc = t.locations.first().cloned();
                // Agents often use a raw path as the title: show "Edit src/x.rs"
                // relative to the project instead.
                let title = match (&loc, t.title.is_empty() || t.title.contains('/')) {
                    (Some((p, _)), true) => {
                        let rel = p
                            .strip_prefix(&app.explorer.root)
                            .unwrap_or(p)
                            .display()
                            .to_string();
                        format!("{} {rel}", verb(&t.kind))
                    }
                    _ if t.title.is_empty() => verb(&t.kind).to_string(),
                    _ => t.title.clone(),
                };
                let title = ellipsize(
                    &format!("{} {title}", tool_icon(&t.kind)),
                    w.saturating_sub(3),
                );
                rows.push(Row {
                    spans: vec![
                        (format!("{mark} "), mark_st),
                        (title, Style::default().fg(pal.fg)),
                    ],
                    target: loc.map(|(p, l)| AgentTarget::OpenFile(p, l)),
                    ..Default::default()
                });
                for d in &t.diffs {
                    let lines = diff_lines(d.old.as_deref(), &d.new);
                    let shown = 12;
                    let target = Some(AgentTarget::OpenFile(d.path.clone(), None));
                    for (sign, text) in lines.iter().take(shown) {
                        let (bg, fgc) = if *sign == '-' {
                            (
                                mix(panel_bg, pal.diff_deleted, 0.28),
                                mix(pal.fg, pal.diff_deleted, 0.3),
                            )
                        } else {
                            (mix(panel_bg, pal.diff_added, 0.28), pal.fg)
                        };
                        for (k, c) in chunk(text, w.saturating_sub(4)).into_iter().enumerate() {
                            let s = if k == 0 {
                                format!("  {sign} {c}")
                            } else {
                                format!("    {c}")
                            };
                            rows.push(Row {
                                spans: vec![(s, Style::default().fg(fgc))],
                                bg: Some(bg),
                                target: target.clone(),
                                ..Default::default()
                            });
                        }
                    }
                    if lines.len() > shown {
                        rows.push(Row::text(
                            format!("    … {} more changed lines", lines.len() - shown),
                            dim,
                        ));
                    }
                }
                if t.diffs.is_empty()
                    && !matches!(t.kind.as_str(), "read" | "edit")
                    && let Some(text) = &t.text
                {
                    let all: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
                    for l in all.iter().take(4) {
                        rows.push(Row::text(ellipsize(&format!("  │ {l}"), w), dim));
                    }
                    if all.len() > 4 {
                        rows.push(Row::text(
                            format!("  │ … {} more lines", all.len() - 4),
                            dim,
                        ));
                    }
                }
            }
            Entry::Plan(items) => {
                rows.push(Row::text("Plan", bold(dim)));
                for (c, s) in items {
                    let (m, st) = match s {
                        PlanStatus::Pending => ("○", Style::default().fg(pal.fg)),
                        PlanStatus::Running => ("◐", Style::default().fg(pal.accent)),
                        PlanStatus::Done => ("✓", Style::default().fg(pal.popup_dim)),
                    };
                    for (k, l) in wrap_text(c, w.saturating_sub(2)).into_iter().enumerate() {
                        let p = if k == 0 { format!("{m} ") } else { "  ".into() };
                        rows.push(Row {
                            spans: vec![(p, Style::default().fg(pal.accent)), (l, st)],
                            ..Default::default()
                        });
                    }
                }
            }
            Entry::Permission(p) => {
                rows.push(Row::blank());
                let warn_bg = mix(panel_bg, pal.warning, 0.15);
                for l in wrap_text(&format!("⚠ Allow: {}?", p.title), w) {
                    rows.push(Row {
                        spans: vec![(l, bold(Style::default().fg(pal.warning)))],
                        bg: Some(warn_bg),
                        ..Default::default()
                    });
                }
                match &p.chosen {
                    Some(c) => rows.push(Row::text(format!("  → {c}"), dim)),
                    None => {
                        // Buttons, wrapped onto as many rows as needed.
                        let mut row = Row {
                            bg: Some(warn_bg),
                            ..Default::default()
                        };
                        let mut x = 0u16;
                        for (n, (id, label, kind)) in p.options.iter().enumerate() {
                            let text = format!(" {} {label} ", n + 1);
                            let tw = width(&text);
                            if x + tw > w as u16 && x > 0 {
                                rows.push(std::mem::take(&mut row));
                                row.bg = Some(warn_bg);
                                x = 0;
                            }
                            let bg = if kind.starts_with("allow") {
                                mix(warn_bg, pal.success, 0.55)
                            } else {
                                mix(warn_bg, pal.error, 0.45)
                            };
                            row.spans.push((text, bold(fgbg(Color::White, bg))));
                            row.spans.push((" ".into(), Style::default()));
                            row.buttons
                                .push((x, tw, AgentTarget::Permission(i, id.clone())));
                            x += tw + 1;
                        }
                        rows.push(row);
                    }
                }
                rows.push(Row::blank());
            }
            Entry::Notice { text, error } => {
                let st = if *error {
                    Style::default().fg(pal.error)
                } else {
                    dim
                };
                for l in wrap_text(&format!("! {text}"), w) {
                    rows.push(Row::text(l, st));
                }
            }
        }
    }
    if a.state == State::Working || (a.state == State::Starting && !a.entries.is_empty()) {
        let secs = a.turn_started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
        rows.push(Row::blank());
        rows.push(Row::text(
            format!("{} {name} is working… {secs}s", spinner()),
            Style::default().fg(pal.accent),
        ));
    } else if let Some((used, _, cost)) = a.usage {
        let price = if cost == 0.0 {
            "free".to_string()
        } else {
            format!("${cost:.4}")
        };
        let k = if used >= 1000 {
            format!("{:.1}k", used as f64 / 1000.0)
        } else {
            used.to_string()
        };
        rows.push(Row::text(format!("— {price} · {k} tokens in context"), dim));
    }
    rows
}

pub fn render(buf: &mut Buffer, area: Rect, app: &mut App) -> Option<Position> {
    let pal = app.theme.palette.clone();
    let compact = app.compact;
    let panel_bg = pal.sidebar_bg;
    fill(buf, area, panel_bg);
    app.layout.agent_panel = area;
    let mut x0 = area.x;
    if !compact {
        for y in area.top()..area.bottom() {
            put(
                buf,
                area.x,
                y,
                "│",
                fgbg(pal.border, panel_bg),
                area.right(),
            );
        }
        x0 += 1;
    }
    let inner = Rect::new(x0, area.y, area.right().saturating_sub(x0), area.height);
    if inner.width < 12 || inner.height < 6 {
        return None;
    }

    // Header: agent, model (tap to change), new chat, close.
    let head_bg = mix(panel_bg, pal.titlebar_bg, 0.6);
    let head = Rect::new(inner.x, inner.y, inner.width, 1);
    fill(buf, head, head_bg);
    let mut x = put(
        buf,
        inner.x + 1,
        inner.y,
        "✦ ",
        bold(fgbg(pal.logo[0], head_bg)),
        head.right(),
    );
    let agent_name = app.agent.name();
    let nx = x;
    x = put(
        buf,
        x,
        inner.y,
        &ellipsize(&agent_name, 14),
        bold(fgbg(pal.fg, head_bg)),
        head.right(),
    );
    if app.agent.available.len() > 1 {
        app.layout
            .agent_targets
            .push((Rect::new(nx, inner.y, x - nx, 1), AgentTarget::PickAgent));
    }
    let right = " ⚙  + New  ✕ ";
    let rx = head.right().saturating_sub(width(right));
    if let Some(label) = app.agent.model_label() {
        // Shorten a provider-qualified model label for compact panels.
        let label = label.rsplit('/').next().unwrap_or(&label).to_string();
        let free = app.agent.model.as_deref().is_some_and(is_free_model);
        let room = rx.saturating_sub(x + 3) as usize;
        let chip = format!(
            " {} ▾",
            ellipsize(&label, room.saturating_sub(if free { 8 } else { 3 }))
        );
        let mx = x + 1;
        x = put(buf, mx, inner.y, &chip, fgbg(pal.popup_dim, head_bg), rx);
        if free && x + 6 < rx {
            x = put(
                buf,
                x + 1,
                inner.y,
                " free ",
                bold(fgbg(pal.accent_fg, pal.success)),
                rx,
            );
        }
        app.layout
            .agent_targets
            .push((Rect::new(mx, inner.y, x - mx, 1), AgentTarget::Model));
    }
    put(buf, rx, inner.y, " ⚙ ", fgbg(pal.fg, head_bg), head.right());
    app.layout
        .agent_targets
        .push((Rect::new(rx, inner.y, 3, 1), AgentTarget::Settings));
    let rx = rx + 3;
    put(
        buf,
        rx,
        inner.y,
        " + New ",
        fgbg(pal.fg, head_bg),
        head.right(),
    );
    app.layout
        .agent_targets
        .push((Rect::new(rx, inner.y, 7, 1), AgentTarget::NewChat));
    put(
        buf,
        rx + 7,
        inner.y,
        " ✕ ",
        fgbg(pal.fg, head_bg),
        head.right(),
    );
    app.layout
        .agent_targets
        .push((Rect::new(rx + 7, inner.y, 3, 1), AgentTarget::Close));

    // Input box (grows to 5 lines), with the attached-context chip above it.
    let text_w = inner.width.saturating_sub(7) as usize;
    let input = app.agent.input.clone();
    let lines: Vec<String> = if input.is_empty() {
        vec![String::new()]
    } else {
        input.split('\n').flat_map(|l| chunk(l, text_w)).collect()
    };
    let input_h = (lines.len() as u16).clamp(1, 5);
    let ctx = app.agent_context_label();
    let ctx_h = u16::from(ctx.is_some());
    let box_top = inner.bottom().saturating_sub(input_h + 1);
    let ctx_y = box_top.saturating_sub(ctx_h);
    let records: Vec<_> = app
        .automation
        .overwatch
        .records
        .iter()
        .filter(|r| r.turn == app.automation.overwatch.turn)
        .collect();
    let overwatch_h = u16::from(app.automation.overwatch.owned_turn || !records.is_empty());
    if overwatch_h > 0 {
        let live = records.iter().filter(|r| !r.status.terminal()).count();
        let waiting = records
            .iter()
            .filter(|r| {
                matches!(
                    r.status,
                    crate::supervision::Status::WaitingChild
                        | crate::supervision::Status::WaitingPermission
                )
            })
            .count();
        let done = records
            .iter()
            .filter(|r| r.status == crate::supervision::Status::Completed)
            .count();
        let failed = records.len().saturating_sub(live + done);
        let remaining = app
            .automation
            .overwatch
            .root_deadline
            .map(|d| {
                format!(
                    " · {}s",
                    d.saturating_duration_since(std::time::Instant::now())
                        .as_secs()
                )
            })
            .unwrap_or_default();
        let label = format!(
            "Overwatch{remaining} · {live} workers · {waiting} wait · {done} done · {failed} stopped"
        );
        put(
            buf,
            inner.x + 1,
            inner.y + 1,
            &ellipsize(&label, inner.width.saturating_sub(2) as usize),
            fgbg(pal.accent, panel_bg),
            inner.right(),
        );
    }
    let body = Rect::new(
        inner.x,
        inner.y + 1 + overwatch_h,
        inner.width,
        ctx_y.saturating_sub(inner.y + 1 + overwatch_h),
    );

    // Transcript.
    let tw = body.width.saturating_sub(2) as usize;
    let rows = build_rows(app, tw, panel_bg);
    let h = body.height as usize;
    let max_scroll = rows.len().saturating_sub(h);
    app.agent.scroll = app.agent.scroll.min(max_scroll);
    let start = max_scroll - app.agent.scroll;
    for (k, row) in rows.iter().skip(start).take(h).enumerate() {
        let y = body.y + k as u16;
        let rx0 = body.x + 1;
        let full = Rect::new(body.x, y, body.width, 1);
        let bg = row.bg.unwrap_or(panel_bg);
        if row.bg.is_some() {
            fill(buf, full, bg);
        }
        let mut cx = rx0;
        for (s, st) in &row.spans {
            let st = if st.bg.is_none() { st.bg(bg) } else { *st };
            cx = put(buf, cx, y, s, st, body.right().saturating_sub(1));
        }
        if let Some(t) = &row.target {
            app.layout.agent_targets.push((full, t.clone()));
        }
        for (bx, bw, t) in &row.buttons {
            app.layout
                .agent_targets
                .push((Rect::new(rx0 + bx, y, *bw, 1), t.clone()));
        }
    }
    if app.agent.scroll > 0 {
        let label = format!(" ↓ {} more ", app.agent.scroll);
        put(
            buf,
            body.right().saturating_sub(width(&label) + 1),
            body.bottom().saturating_sub(1),
            &label,
            bold(fgbg(pal.accent_fg, pal.accent)),
            body.right(),
        );
    }

    if let Some(c) = ctx {
        put(
            buf,
            inner.x + 1,
            ctx_y,
            &ellipsize(&format!("@ {c}"), inner.width as usize - 2),
            fgbg(pal.popup_dim, panel_bg),
            inner.right(),
        );
    }

    // Input.
    let ib = Rect::new(inner.x, box_top, inner.width, input_h + 1);
    let focused = app.focus == Focus::Agent;
    let border = if focused { pal.accent } else { pal.border };
    put(
        buf,
        ib.x,
        ib.y,
        &"─".repeat(ib.width as usize),
        fgbg(border, panel_bg),
        ib.right(),
    );
    let field = Rect::new(ib.x, ib.y + 1, ib.width, input_h);
    fill(buf, field, pal.input_bg);
    app.layout.agent_input = field;
    put(
        buf,
        field.x + 1,
        field.y,
        "›",
        bold(fgbg(pal.accent, pal.input_bg)),
        field.right(),
    );
    let working = app.agent.is_working();
    let (btn, btn_bg, target) = if working {
        (" ■ ", pal.error, AgentTarget::Stop)
    } else {
        (
            " ➤ ",
            if input.trim().is_empty() {
                mix(pal.input_bg, pal.fg, 0.15)
            } else {
                pal.accent
            },
            AgentTarget::Send,
        )
    };
    let bx = field.right().saturating_sub(4);
    put(
        buf,
        bx,
        field.y,
        btn,
        bold(fgbg(pal.accent_fg, btn_bg)),
        field.right(),
    );
    app.layout
        .agent_targets
        .push((Rect::new(bx, field.y, 3, 1), target));
    let tx = field.x + 3;
    if input.is_empty() {
        let hint = if app.agent.pending_permission().is_some() {
            "Tap a button above, or press 1-3".to_string()
        } else {
            format!("Ask {agent_name} to change or explain code…")
        };
        put(
            buf,
            tx,
            field.y,
            &ellipsize(&hint, text_w),
            fgbg(pal.popup_dim, pal.input_bg).add_modifier(Modifier::ITALIC),
            bx,
        );
    } else {
        let skip = lines.len().saturating_sub(input_h as usize);
        for (k, l) in lines.iter().skip(skip).enumerate() {
            put(
                buf,
                tx,
                field.y + k as u16,
                l,
                fgbg(pal.fg, pal.input_bg),
                bx,
            );
        }
    }
    if !focused {
        return None;
    }
    // Caret: walk the wrapped lines to the cursor.
    let before: String = input.chars().take(app.agent.cursor).collect();
    let mut row = 0usize;
    let mut col = 0usize;
    for (k, part) in before.split('\n').enumerate() {
        let chunks = chunk(part, text_w);
        if k > 0 {
            row += 1;
        }
        row += chunks.len() - 1;
        col = UnicodeWidthStr::width(chunks.last().map(String::as_str).unwrap_or(""));
    }
    let total_rows = lines.len();
    let skip = total_rows.saturating_sub(input_h as usize);
    let vis = row.checked_sub(skip)?;
    Some(Position::new(tx + col as u16, field.y + vis as u16))
}
