//! Themes: a syntect syntax theme paired with a UI palette for the editor chrome.
//!
//! The flagship theme reproduces the Visual Studio 2022 dark C# editor colors;
//! the rest come from the curated `two-face` (bat) collection, with UI chrome
//! derived from each theme's own editor settings.

use std::str::FromStr;

use ratatui::style::Color;
use syntect::highlighting::{
    Color as SColor, FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use two_face::theme::EmbeddedThemeName;

/// Colors for everything that is not syntax: gutters, tabs, panels, popups.
#[derive(Clone, Debug)]
pub struct Palette {
    pub bg: Color,
    pub fg: Color,
    pub gutter_fg: Color,
    pub gutter_fg_active: Color,
    pub line_highlight: Color,
    pub selection: Color,
    pub word_highlight: Color,
    pub find_match: Color,
    pub find_match_current: Color,
    pub bracket_match: Color,
    pub indent_guide: Color,
    pub indent_guide_active: Color,
    pub whitespace: Color,
    pub sidebar_bg: Color,
    pub sidebar_fg: Color,
    pub sidebar_dim: Color,
    pub sidebar_selected: Color,
    pub titlebar_bg: Color,
    pub tabbar_bg: Color,
    pub tab_active_bg: Color,
    pub tab_inactive_bg: Color,
    pub tab_active_fg: Color,
    pub tab_inactive_fg: Color,
    pub accent: Color,
    pub accent_fg: Color,
    pub status_bg: Color,
    pub status_fg: Color,
    pub border: Color,
    pub popup_bg: Color,
    pub popup_fg: Color,
    pub popup_dim: Color,
    pub popup_selected: Color,
    pub match_fg: Color,
    pub input_bg: Color,
    pub minimap_viewport: Color,
    pub scrollbar_track: Color,
    pub scrollbar_thumb: Color,
    pub diff_added: Color,
    pub diff_modified: Color,
    pub diff_deleted: Color,
    pub brackets: [Color; 3],
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub logo: [Color; 2],
}

pub struct ThemeEntry {
    pub name: String,
    pub syntax: Theme,
    pub palette: Palette,
}

pub fn theme_names() -> Vec<&'static str> {
    let mut v = vec!["Visual Studio Dark", "Termed Neon"];
    v.extend(EMBEDDED.iter().map(|(n, _)| *n));
    v
}

const EMBEDDED: &[(&str, EmbeddedThemeName)] = &[
    ("Dracula", EmbeddedThemeName::Dracula),
    ("Catppuccin Mocha", EmbeddedThemeName::CatppuccinMocha),
    ("One Half Dark", EmbeddedThemeName::OneHalfDark),
    ("Two Dark", EmbeddedThemeName::TwoDark),
    ("Nord", EmbeddedThemeName::Nord),
    ("Monokai Extended", EmbeddedThemeName::MonokaiExtended),
    ("Gruvbox Dark", EmbeddedThemeName::GruvboxDark),
    ("Solarized Dark", EmbeddedThemeName::SolarizedDark),
    ("Sublime Snazzy", EmbeddedThemeName::SublimeSnazzy),
    ("1337", EmbeddedThemeName::Leet),
    ("Catppuccin Latte", EmbeddedThemeName::CatppuccinLatte),
    ("GitHub Light", EmbeddedThemeName::Github),
];

pub fn load(name: &str) -> Option<ThemeEntry> {
    let lower = name.to_lowercase();
    if lower == "visual studio dark" || lower == "vs" || lower == "vs-dark" {
        return Some(visual_studio_dark());
    }
    if lower == "termed neon" || lower == "neon" {
        return Some(termed_neon());
    }
    let (n, embedded) = EMBEDDED.iter().find(|(n, _)| n.to_lowercase() == lower)?;
    let set = two_face::theme::extra();
    let syntax = set.get(*embedded).clone();
    let palette = derive_palette(&syntax);
    Some(ThemeEntry {
        name: n.to_string(),
        syntax,
        palette,
    })
}

// ------------------------------------------------------------------ helpers

pub fn hex(s: &str) -> Color {
    let (r, g, b) = hex_rgb(s);
    Color::Rgb(r, g, b)
}

fn hex_rgb(s: &str) -> (u8, u8, u8) {
    let s = s.trim_start_matches('#');
    let p = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).unwrap_or(0);
    (p(0), p(2), p(4))
}

fn shex(s: &str) -> SColor {
    let (r, g, b) = hex_rgb(s);
    SColor { r, g, b, a: 0xff }
}

pub fn rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::White => (255, 255, 255),
        _ => (128, 128, 128),
    }
}

/// Linear blend: `t` = 0 gives `a`, 1 gives `b`.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let (ar, ag, ab) = rgb(a);
    let (br, bg, bb) = rgb(b);
    let l = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color::Rgb(l(ar, br), l(ag, bg), l(ab, bb))
}

pub fn luminance(c: Color) -> f32 {
    let (r, g, b) = rgb(c);
    (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0
}

/// Convert a syntect color (possibly translucent) to an opaque color over `bg`.
pub fn from_syntect(c: SColor, bg: Color) -> Color {
    let base = Color::Rgb(c.r, c.g, c.b);
    if c.a == 0xff {
        base
    } else {
        mix(bg, base, c.a as f32 / 255.0)
    }
}

fn rule(selectors: &str, fg: &str, style: Option<FontStyle>) -> ThemeItem {
    ThemeItem {
        scope: ScopeSelectors::from_str(selectors).expect("valid scope selector"),
        style: StyleModifier {
            foreground: Some(shex(fg)),
            background: None,
            font_style: style,
        },
    }
}

fn settings(bg: &str, fg: &str, line: &str, sel: &str, caret: &str) -> ThemeSettings {
    ThemeSettings {
        foreground: Some(shex(fg)),
        background: Some(shex(bg)),
        caret: Some(shex(caret)),
        line_highlight: Some(shex(line)),
        selection: Some(shex(sel)),
        ..ThemeSettings::default()
    }
}

// ------------------------------------------------------------ built-in themes

/// Visual Studio 2022 "Dark" C# editor colors, mapped onto TextMate scopes so
/// that every language gets the same look.
fn visual_studio_dark() -> ThemeEntry {
    let b = Some(FontStyle::BOLD);
    let i = Some(FontStyle::ITALIC);
    let scopes = vec![
        rule("comment, punctuation.definition.comment", "#57A64A", None),
        rule(
            "comment.block.documentation, comment.line.documentation",
            "#608B4E",
            None,
        ),
        rule(
            "string, punctuation.definition.string, string.quoted",
            "#D69D85",
            None,
        ),
        rule("string.regexp", "#D16969", None),
        rule(
            "constant.character.escape, constant.character",
            "#E8C9BB",
            None,
        ),
        rule("constant.numeric", "#B5CEA8", None),
        rule("constant.language, variable.language", "#569CD6", None),
        rule(
            "constant.other, variable.other.constant, entity.name.constant",
            "#4FC1FF",
            None,
        ),
        rule(
            "keyword, storage, storage.type, storage.modifier, keyword.other",
            "#569CD6",
            None,
        ),
        rule(
            "keyword.control, keyword.control.flow, keyword.control.conditional, keyword.control.loop, keyword.control.import, keyword.control.return",
            "#D8A0DF",
            None,
        ),
        rule(
            "keyword.operator, punctuation.separator, punctuation.terminator, punctuation.accessor",
            "#B4B4B4",
            None,
        ),
        rule(
            "keyword.operator.new, keyword.operator.word, keyword.operator.logical.python, keyword.operator.expression, keyword.operator.sizeof, keyword.operator.typeof",
            "#569CD6",
            None,
        ),
        rule(
            "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.name.interface, entity.name.trait, entity.other.inherited-class, support.type, support.class, storage.type.class.cs, meta.type",
            "#4EC9B0",
            None,
        ),
        rule(
            "entity.name.type.interface, entity.name.interface",
            "#B8D7A3",
            None,
        ),
        rule(
            "entity.name.namespace, entity.name.module, meta.path entity.name",
            "#DCDCDC",
            None,
        ),
        rule(
            "entity.name.function, support.function, meta.function-call entity.name, variable.function",
            "#DCDCAA",
            None,
        ),
        rule(
            "variable.parameter, meta.parameter variable",
            "#9CDCFE",
            None,
        ),
        rule(
            "variable, variable.other.readwrite, variable.other.member, meta.property",
            "#DADADA",
            None,
        ),
        rule(
            "variable.other.property, variable.other.object.property, support.variable.property",
            "#DADADA",
            None,
        ),
        rule(
            "entity.name.tag, punctuation.definition.tag",
            "#569CD6",
            None,
        ),
        rule("entity.other.attribute-name", "#92CAF4", None),
        rule(
            "support.type.property-name, meta.mapping.key string, entity.name.tag.yaml",
            "#9CDCFE",
            None,
        ),
        rule(
            "entity.name.label, entity.name.lifetime, storage.modifier.lifetime",
            "#C8C8C8",
            None,
        ),
        rule(
            "meta.preprocessor, keyword.preprocessor, keyword.control.directive, meta.attribute, meta.annotation",
            "#9B9B9B",
            None,
        ),
        rule(
            "entity.name.function.macro, support.function.macro, entity.name.macro",
            "#BEB7FF",
            None,
        ),
        rule("markup.heading, markup.heading entity.name", "#569CD6", b),
        rule("markup.bold", "#D4D4D4", b),
        rule("markup.italic", "#D4D4D4", i),
        rule("markup.inline.raw, markup.raw", "#D69D85", None),
        rule("markup.underline.link, markup.link", "#4EC9B0", None),
        rule("markup.inserted", "#B5CEA8", None),
        rule("markup.deleted", "#D16969", None),
        rule("markup.changed", "#569CD6", None),
        rule("invalid, invalid.illegal", "#F44747", None),
    ];
    let syntax = Theme {
        name: Some("Visual Studio Dark".into()),
        author: Some("termed".into()),
        settings: settings("#1E1E1E", "#DCDCDC", "#2A2A2A", "#264F78", "#DCDCDC"),
        scopes,
    };
    let palette = Palette {
        bg: hex("#1E1E1E"),
        fg: hex("#DCDCDC"),
        gutter_fg: hex("#2B91AF"),
        gutter_fg_active: hex("#CFE6F0"),
        line_highlight: hex("#262628"),
        selection: hex("#264F78"),
        word_highlight: hex("#343A40"),
        find_match: hex("#653306"),
        find_match_current: hex("#9E6A03"),
        bracket_match: hex("#0E4A2D"),
        indent_guide: hex("#353535"),
        indent_guide_active: hex("#6E6E6E"),
        whitespace: hex("#4A4A4A"),
        sidebar_bg: hex("#252526"),
        sidebar_fg: hex("#CCCCCC"),
        sidebar_dim: hex("#8B8B8B"),
        sidebar_selected: hex("#094771"),
        titlebar_bg: hex("#1A1A1C"),
        tabbar_bg: hex("#2D2D30"),
        tab_active_bg: hex("#1E1E1E"),
        tab_inactive_bg: hex("#2D2D30"),
        tab_active_fg: hex("#FFFFFF"),
        tab_inactive_fg: hex("#969696"),
        accent: hex("#007ACC"),
        accent_fg: hex("#FFFFFF"),
        status_bg: hex("#007ACC"),
        status_fg: hex("#FFFFFF"),
        border: hex("#3F3F46"),
        popup_bg: hex("#252526"),
        popup_fg: hex("#CCCCCC"),
        popup_dim: hex("#8B8B8B"),
        popup_selected: hex("#04395E"),
        match_fg: hex("#2AAAFF"),
        input_bg: hex("#3C3C3C"),
        minimap_viewport: hex("#2E2E30"),
        scrollbar_track: hex("#1E1E1E"),
        scrollbar_thumb: hex("#4E4E52"),
        diff_added: hex("#57AB5A"),
        diff_modified: hex("#1B81A8"),
        diff_deleted: hex("#C74E39"),
        brackets: [hex("#FFD700"), hex("#DA70D6"), hex("#179FFF")],
        error: hex("#F14C4C"),
        warning: hex("#CCA700"),
        success: hex("#89D185"),
        logo: [hex("#00C8FF"), hex("#B04CFF")],
    };
    ThemeEntry {
        name: "Visual Studio Dark".into(),
        syntax,
        palette,
    }
}

/// A high-contrast "terminal hacker" theme: deep black, phosphor greens and cyans.
fn termed_neon() -> ThemeEntry {
    let i = Some(FontStyle::ITALIC);
    let b = Some(FontStyle::BOLD);
    let scopes = vec![
        rule("comment, punctuation.definition.comment", "#3D6B4F", i),
        rule("string, punctuation.definition.string", "#FFE66D", None),
        rule("constant.character.escape, string.regexp", "#FF9E64", None),
        rule("constant.numeric, constant.language", "#FF6AC1", None),
        rule("constant.other, variable.other.constant", "#FF6AC1", None),
        rule(
            "keyword, storage, storage.type, storage.modifier",
            "#00FF9C",
            None,
        ),
        rule("keyword.control", "#00FF9C", b),
        rule("keyword.operator, punctuation", "#7FDBCA", None),
        rule(
            "entity.name.type, entity.name.class, support.type, support.class, entity.other.inherited-class",
            "#00D9FF",
            None,
        ),
        rule(
            "entity.name.function, support.function, variable.function",
            "#C792EA",
            None,
        ),
        rule("variable.parameter", "#FFB86C", i),
        rule("variable, meta.property", "#D6FFE6", None),
        rule("variable.language", "#00FF9C", i),
        rule("entity.name.tag", "#00FF9C", None),
        rule(
            "entity.other.attribute-name, support.type.property-name",
            "#00D9FF",
            None,
        ),
        rule(
            "meta.preprocessor, meta.attribute, meta.annotation",
            "#5F8F7A",
            None,
        ),
        rule("markup.heading", "#00FF9C", b),
        rule("invalid", "#FF3860", None),
    ];
    let syntax = Theme {
        name: Some("Termed Neon".into()),
        author: Some("termed".into()),
        settings: settings("#0A0E0D", "#C8F7DC", "#0F1A16", "#11403A", "#00FF9C"),
        scopes,
    };
    let palette = Palette {
        bg: hex("#0A0E0D"),
        fg: hex("#C8F7DC"),
        gutter_fg: hex("#2C5A47"),
        gutter_fg_active: hex("#00FF9C"),
        line_highlight: hex("#0F1A16"),
        selection: hex("#11403A"),
        word_highlight: hex("#16302A"),
        find_match: hex("#4A3A00"),
        find_match_current: hex("#8A6A00"),
        bracket_match: hex("#1D4D3A"),
        indent_guide: hex("#15261F"),
        indent_guide_active: hex("#2E6B52"),
        whitespace: hex("#1F3A2F"),
        sidebar_bg: hex("#070A09"),
        sidebar_fg: hex("#9FD8B8"),
        sidebar_dim: hex("#4F7A64"),
        sidebar_selected: hex("#0F3B2C"),
        titlebar_bg: hex("#050706"),
        tabbar_bg: hex("#070A09"),
        tab_active_bg: hex("#0A0E0D"),
        tab_inactive_bg: hex("#070A09"),
        tab_active_fg: hex("#00FF9C"),
        tab_inactive_fg: hex("#4F7A64"),
        accent: hex("#00FF9C"),
        accent_fg: hex("#001A0F"),
        status_bg: hex("#00C77A"),
        status_fg: hex("#001A0F"),
        border: hex("#1D3A2E"),
        popup_bg: hex("#0C1311"),
        popup_fg: hex("#C8F7DC"),
        popup_dim: hex("#4F7A64"),
        popup_selected: hex("#123D2E"),
        match_fg: hex("#00FF9C"),
        input_bg: hex("#132420"),
        minimap_viewport: hex("#11201B"),
        scrollbar_track: hex("#0A0E0D"),
        scrollbar_thumb: hex("#1D3A2E"),
        diff_added: hex("#00FF9C"),
        diff_modified: hex("#00D9FF"),
        diff_deleted: hex("#FF3860"),
        brackets: [hex("#FFE66D"), hex("#FF6AC1"), hex("#00D9FF")],
        error: hex("#FF3860"),
        warning: hex("#FFE66D"),
        success: hex("#00FF9C"),
        logo: [hex("#00FF9C"), hex("#00D9FF")],
    };
    ThemeEntry {
        name: "Termed Neon".into(),
        syntax,
        palette,
    }
}

/// Build editor chrome colors from a TextMate theme's own settings.
fn derive_palette(theme: &Theme) -> Palette {
    let s = &theme.settings;
    let bg = s
        .background
        .map(|c| from_syntect(c, Color::Black))
        .unwrap_or(hex("#1E1E1E"));
    let fg = s
        .foreground
        .map(|c| from_syntect(c, bg))
        .unwrap_or(hex("#D4D4D4"));
    let dark = luminance(bg) < 0.5;
    let (shade, lift) = if dark {
        (Color::Black, Color::White)
    } else {
        (Color::White, Color::Black)
    };
    let pick =
        |c: Option<SColor>, fallback: Color| c.map(|c| from_syntect(c, bg)).unwrap_or(fallback);

    let line_highlight = pick(s.line_highlight, mix(bg, lift, 0.05));
    let selection = pick(s.selection, mix(bg, hex("#3399FF"), 0.35));
    // A strong scope color makes a good accent (keywords are usually the signature hue).
    let accent = theme
        .scopes
        .iter()
        .find(|item| format!("{:?}", item.scope).contains("keyword"))
        .and_then(|item| item.style.foreground)
        .map(|c| from_syntect(c, bg))
        .unwrap_or(hex("#007ACC"));
    let accent_fg = if luminance(accent) > 0.55 {
        mix(bg, Color::Black, 0.6)
    } else {
        Color::White
    };
    let sidebar_bg = mix(bg, shade, if dark { 0.18 } else { 0.03 });
    let gutter_fg = pick(s.gutter_foreground, mix(bg, fg, 0.35));

    Palette {
        bg,
        fg,
        gutter_fg,
        gutter_fg_active: mix(fg, lift, 0.2),
        line_highlight,
        selection,
        word_highlight: mix(bg, selection, 0.45),
        find_match: mix(bg, hex("#D18616"), 0.35),
        find_match_current: mix(bg, hex("#D18616"), 0.7),
        bracket_match: mix(bg, hex("#3FB950"), 0.3),
        indent_guide: mix(bg, fg, 0.1),
        indent_guide_active: mix(bg, fg, 0.35),
        whitespace: mix(bg, fg, 0.22),
        sidebar_bg,
        sidebar_fg: mix(fg, sidebar_bg, 0.1),
        sidebar_dim: mix(fg, sidebar_bg, 0.45),
        sidebar_selected: mix(sidebar_bg, accent, 0.3),
        titlebar_bg: mix(bg, shade, if dark { 0.3 } else { 0.06 }),
        tabbar_bg: sidebar_bg,
        tab_active_bg: bg,
        tab_inactive_bg: sidebar_bg,
        tab_active_fg: mix(fg, lift, 0.3),
        tab_inactive_fg: mix(fg, sidebar_bg, 0.45),
        accent,
        accent_fg,
        status_bg: accent,
        status_fg: accent_fg,
        border: mix(bg, fg, 0.18),
        popup_bg: sidebar_bg,
        popup_fg: fg,
        popup_dim: mix(fg, sidebar_bg, 0.45),
        popup_selected: mix(sidebar_bg, accent, 0.28),
        match_fg: accent,
        input_bg: mix(bg, lift, 0.08),
        minimap_viewport: mix(bg, lift, 0.07),
        scrollbar_track: bg,
        scrollbar_thumb: mix(bg, fg, 0.22),
        diff_added: hex("#57AB5A"),
        diff_modified: hex("#1B81A8"),
        diff_deleted: hex("#C74E39"),
        brackets: if dark {
            [hex("#FFD700"), hex("#DA70D6"), hex("#179FFF")]
        } else {
            [hex("#0431FA"), hex("#319331"), hex("#7B3814")]
        },
        error: hex("#F14C4C"),
        warning: hex("#CCA700"),
        success: hex("#89D185"),
        logo: [accent, mix(accent, hex("#B04CFF"), 0.7)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_loads() {
        for name in theme_names() {
            let t = load(name).unwrap_or_else(|| panic!("theme {name} failed to load"));
            assert_eq!(t.name, name);
        }
    }
}
