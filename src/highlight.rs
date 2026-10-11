//! Incremental syntax highlighting on top of syntect (Sublime Text / TextMate
//! grammars, ~200 languages via the `two-face` bundle).
//!
//! Parser state is checkpointed at the start of every line, so an edit only
//! re-highlights from the first changed line down to the bottom of the
//! viewport. While highlighting we also track the scope stack to find
//! brackets that are real code (not inside strings or comments), which powers
//! bracket-pair colorization and string-aware bracket matching.

use std::sync::OnceLock;
use std::time::Instant;

use ratatui::style::Color;
use ropey::Rope;
use syntect::highlighting::{FontStyle, HighlightIterator, HighlightState, Highlighter, Theme};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use crate::theme::from_syntect;

/// Lines longer than this are shown without highlighting (minified files, data blobs).
const MAX_LINE_BYTES: usize = 4096;

pub fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

/// Pick a grammar from the file name, falling back to the first line (shebangs, modelines).
pub fn detect_syntax(path: Option<&std::path::Path>, first_line: &str) -> &'static SyntaxReference {
    let ss = syntax_set();
    if let Some(p) = path {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(s) = ss.find_syntax_by_extension(&name) {
            return s;
        }
        if let Some(ext) = p.extension().map(|e| e.to_string_lossy().into_owned()) {
            if let Some(s) = ss.find_syntax_by_extension(&ext) {
                return s;
            }
            let alias = match ext.as_str() {
                "csx" => Some("cs"),
                "tsx" | "mts" | "cts" => Some("ts"),
                "jsx" | "mjs" | "cjs" => Some("js"),
                "h" | "hh" | "hxx" => Some("cpp"),
                "kts" => Some("kt"),
                _ => None,
            };
            if let Some(s) = alias.and_then(|a| ss.find_syntax_by_extension(a)) {
                return s;
            }
        }
    }
    ss.find_syntax_by_first_line(first_line)
        .unwrap_or_else(|| ss.find_syntax_plain_text())
}

#[derive(Clone, Debug)]
pub struct StyledSpan {
    /// Char range within the line.
    pub start: usize,
    pub end: usize,
    pub fg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bracket {
    pub col: usize,
    pub ch: char,
    /// Nesting depth (0 = outermost) of the pair this bracket belongs to.
    pub depth: usize,
    pub open: bool,
}

#[derive(Clone, Debug, Default)]
pub struct LineHighlight {
    pub spans: Vec<StyledSpan>,
    pub brackets: Vec<Bracket>,
}

#[derive(Clone)]
struct Checkpoint {
    parse: ParseState,
    hl: HighlightState,
    depth: usize,
}

pub struct HighlightCache {
    syntax: &'static SyntaxReference,
    /// `starts[i]` is the parser state at the beginning of line `i`.
    starts: Vec<Checkpoint>,
    lines: Vec<LineHighlight>,
    /// Provisional highlighting for a region far beyond the exact frontier,
    /// parsed from a fresh state (first line, lines). Lets a jump to the end
    /// of a huge file render instantly while exact highlighting catches up.
    approx: Option<(usize, Vec<LineHighlight>)>,
}

fn non_code_scopes() -> &'static [Scope; 2] {
    static S: OnceLock<[Scope; 2]> = OnceLock::new();
    S.get_or_init(|| {
        [
            Scope::new("string").unwrap(),
            Scope::new("comment").unwrap(),
        ]
    })
}

fn is_code(stack: &ScopeStack) -> bool {
    let [string, comment] = non_code_scopes();
    !stack
        .as_slice()
        .iter()
        .any(|s| string.is_prefix_of(*s) || comment.is_prefix_of(*s))
}

impl HighlightCache {
    pub fn new(syntax: &'static SyntaxReference) -> Self {
        Self {
            syntax,
            starts: Vec::new(),
            lines: Vec::new(),
            approx: None,
        }
    }

    pub fn syntax(&self) -> &'static SyntaxReference {
        self.syntax
    }

    pub fn set_syntax(&mut self, syntax: &'static SyntaxReference) {
        self.syntax = syntax;
        self.invalidate_all();
    }

    pub fn invalidate_all(&mut self) {
        self.starts.clear();
        self.lines.clear();
        self.approx = None;
    }

    /// Drop everything from `line` downwards.
    pub fn invalidate_from(&mut self, line: usize) {
        self.lines.truncate(line);
        self.starts.truncate(line + 1);
        if self
            .approx
            .as_ref()
            .is_some_and(|(s, v)| line < s + v.len())
        {
            self.approx = None;
        }
    }

    pub fn line(&self, line: usize) -> Option<&LineHighlight> {
        self.lines.get(line).or_else(|| {
            let (start, v) = self.approx.as_ref()?;
            v.get(line.checked_sub(*start)?)
        })
    }

    /// Highlight what the viewport `top..=bottom` needs, spending at most
    /// until `deadline` on exact highlighting. Returns false if exact
    /// highlighting is still behind (call again on the next frame).
    pub fn ensure_view(
        &mut self,
        rope: &Rope,
        top: usize,
        bottom: usize,
        theme: &Theme,
        bg: Color,
        deadline: Instant,
    ) -> bool {
        if self.ensure_until(rope, bottom, theme, bg, Some(deadline)) {
            self.approx = None;
            return true;
        }
        let total = rope.len_lines();
        let bottom = bottom.min(total.saturating_sub(1));
        let start = top.saturating_sub(64).max(self.lines.len());
        let covered = self
            .approx
            .as_ref()
            .is_some_and(|(s, v)| *s == start && s + v.len() > bottom);
        if !covered && start <= bottom {
            let highlighter = Highlighter::new(theme);
            let mut cp = Checkpoint {
                parse: ParseState::new(self.syntax),
                hl: HighlightState::new(&highlighter, ScopeStack::new()),
                depth: 0,
            };
            let mut text = String::new();
            let v = (start..=bottom)
                .map(|i| self.step(rope, i, &mut cp, &highlighter, theme, bg, &mut text))
                .collect();
            self.approx = Some((start, v));
        }
        false
    }

    /// Make sure lines `0..=upto` are highlighted.
    pub fn ensure(&mut self, rope: &Rope, upto: usize, theme: &Theme, bg: Color) {
        self.ensure_until(rope, upto, theme, bg, None);
    }

    fn ensure_until(
        &mut self,
        rope: &Rope,
        upto: usize,
        theme: &Theme,
        bg: Color,
        deadline: Option<Instant>,
    ) -> bool {
        let total = rope.len_lines();
        let upto = upto.min(total.saturating_sub(1));
        if self.lines.len() > upto {
            return true;
        }
        let highlighter = Highlighter::new(theme);
        if self.starts.is_empty() {
            self.starts.push(Checkpoint {
                parse: ParseState::new(self.syntax),
                hl: HighlightState::new(&highlighter, ScopeStack::new()),
                depth: 0,
            });
        }
        let mut text = String::new();
        while self.lines.len() <= upto {
            let i = self.lines.len();
            let mut cp = self.starts[i].clone();
            let lh = self.step(rope, i, &mut cp, &highlighter, theme, bg, &mut text);
            self.lines.push(lh);
            self.starts.push(cp);
            if i.is_multiple_of(64) && deadline.is_some_and(|d| Instant::now() >= d) {
                return self.lines.len() > upto;
            }
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn step(
        &self,
        rope: &Rope,
        i: usize,
        cp: &mut Checkpoint,
        highlighter: &Highlighter<'_>,
        theme: &Theme,
        bg: Color,
        text: &mut String,
    ) -> LineHighlight {
        text.clear();
        for chunk in rope.line(i).chunks() {
            text.push_str(chunk);
        }
        if !text.ends_with('\n') {
            text.push('\n');
        }
        if text.len() > MAX_LINE_BYTES {
            let fg = theme
                .settings
                .foreground
                .map(|c| from_syntect(c, bg))
                .unwrap_or(Color::Reset);
            let end = text.chars().count();
            return LineHighlight {
                spans: vec![StyledSpan {
                    start: 0,
                    end,
                    fg,
                    bold: false,
                    italic: false,
                    underline: false,
                }],
                brackets: Vec::new(),
            };
        }
        highlight_line(text, cp, highlighter, syntax_set(), bg)
    }
}

fn highlight_line(
    text: &str,
    cp: &mut Checkpoint,
    highlighter: &Highlighter<'_>,
    ss: &SyntaxSet,
    bg: Color,
) -> LineHighlight {
    let ops = cp.parse.parse_line(text, ss).unwrap_or_default();

    // Pass 1: bracket detection with scope awareness.
    let mut brackets = Vec::new();
    let mut stack = cp.hl.path.clone();
    let mut op_iter = ops.iter().peekable();
    for (col, (byte, ch)) in text.char_indices().enumerate() {
        while let Some((pos, op)) = op_iter.peek() {
            if *pos > byte {
                break;
            }
            let _ = stack.apply(op);
            op_iter.next();
        }
        let (open, close) = match ch {
            '(' | '[' | '{' => (true, false),
            ')' | ']' | '}' => (false, true),
            _ => (false, false),
        };
        if (open || close) && is_code(&stack) {
            if open {
                brackets.push(Bracket {
                    col,
                    ch,
                    depth: cp.depth,
                    open: true,
                });
                cp.depth += 1;
            } else {
                cp.depth = cp.depth.saturating_sub(1);
                brackets.push(Bracket {
                    col,
                    ch,
                    depth: cp.depth,
                    open: false,
                });
            }
        }
    }

    // Pass 2: colors.
    let mut spans = Vec::new();
    let mut col = 0usize;
    for (style, piece) in HighlightIterator::new(&mut cp.hl, &ops, text, highlighter) {
        let piece = piece.strip_suffix('\n').unwrap_or(piece);
        let n = piece.chars().count();
        if n == 0 {
            continue;
        }
        spans.push(StyledSpan {
            start: col,
            end: col + n,
            fg: from_syntect(style.foreground, bg),
            bold: style.font_style.contains(FontStyle::BOLD),
            italic: style.font_style.contains(FontStyle::ITALIC),
            underline: style.font_style.contains(FontStyle::UNDERLINE),
        });
        col += n;
    }
    LineHighlight { spans, brackets }
}

/// Find the bracket matching the one at (`line`, `col`), using the
/// highlighter's string/comment-aware bracket lists.
pub fn match_bracket(cache: &HighlightCache, line: usize, col: usize) -> Option<(usize, usize)> {
    let lh = cache.line(line)?;
    let b = lh.brackets.iter().find(|b| b.col == col)?;
    let pair = |c: char| match c {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        ')' => '(',
        ']' => '[',
        '}' => '{',
        _ => c,
    };
    let want = pair(b.ch);
    if b.open {
        let mut l = line;
        let mut first = true;
        while let Some(lh) = cache.line(l) {
            for x in &lh.brackets {
                if first && x.col <= col {
                    continue;
                }
                if !x.open && x.depth == b.depth {
                    return (x.ch == want).then_some((l, x.col));
                }
            }
            first = false;
            l += 1;
        }
    } else {
        let mut l = line as isize;
        let mut first = true;
        while l >= 0 {
            let lh = cache.line(l as usize)?;
            for x in lh.brackets.iter().rev() {
                if first && x.col >= col {
                    continue;
                }
                if x.open && x.depth == b.depth {
                    return (x.ch == want).then_some((l as usize, x.col));
                }
            }
            first = false;
            l -= 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_for(src: &str, ext: &str) -> (HighlightCache, Rope) {
        let rope = Rope::from_str(src);
        let path = std::path::PathBuf::from(format!("x.{ext}"));
        let mut c = HighlightCache::new(detect_syntax(Some(&path), ""));
        let theme = crate::theme::load("Visual Studio Dark").unwrap();
        c.ensure(&rope, usize::MAX, &theme.syntax, theme.palette.bg);
        (c, rope)
    }

    #[test]
    fn detects_common_languages() {
        for (ext, name) in [
            ("cs", "C#"),
            ("rs", "Rust"),
            ("py", "Python"),
            ("ts", "TypeScript"),
            ("go", "Go"),
        ] {
            let s = detect_syntax(Some(std::path::Path::new(&format!("a.{ext}"))), "");
            assert_eq!(s.name, name, "extension {ext}");
        }
        assert_eq!(detect_syntax(None, "#!/usr/bin/env python3").name, "Python");
    }

    #[test]
    fn brackets_in_strings_are_ignored() {
        let (c, _) = cache_for("fn main() {\n    let s = \"(\";\n}\n", "rs");
        let l1 = c.line(1).unwrap();
        assert!(l1.brackets.is_empty(), "{:?}", l1.brackets);
        assert_eq!(match_bracket(&c, 0, 10), Some((2, 0)));
        assert_eq!(match_bracket(&c, 2, 0), Some((0, 10)));
    }

    #[test]
    fn keywords_get_visual_studio_blue() {
        let (c, _) = cache_for("public class Foo { }\n", "cs");
        let first = &c.line(0).unwrap().spans[0];
        assert_eq!(first.fg, crate::theme::hex("#569CD6"));
    }
}
