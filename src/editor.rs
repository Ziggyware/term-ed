//! An open editor tab: document + highlighter + caret/selection + viewport,
//! and all the Visual Studio style editing commands.

use std::path::PathBuf;

use unicode_width::UnicodeWidthChar;

use crate::document::{Document, EditKind, Indent, Selection};
use crate::highlight::{self, HighlightCache};

pub struct Editor {
    pub doc: Document,
    pub hl: HighlightCache,
    pub sel: Selection,
    /// Desired display column for vertical movement.
    pub preferred_col: Option<usize>,
    pub scroll_line: usize,
    pub scroll_col: usize,
    /// Text area size from the last render (for paging and scroll-into-view).
    pub view_height: usize,
    pub view_width: usize,
    /// Cached gutter change markers, keyed by document version.
    pub diff_cache: Option<(u64, Vec<LineChange>)>,
    /// Exact highlighting has not caught up with the viewport yet.
    pub hl_pending: bool,
    /// Soft-wrap width in columns (set by the renderer when word wrap is on).
    pub wrap_width: Option<usize>,
    /// First visible wrapped row of `scroll_line`.
    pub scroll_seg: usize,
    /// Box (column) selection, Visual Studio's Alt+Shift+arrows / Alt+drag.
    pub block: Option<BoxSel>,
}

/// A rectangular selection in display columns. Columns may lie past the end
/// of short lines ("virtual space"); typing pads those lines with spaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoxSel {
    pub anchor_line: usize,
    pub anchor_col: usize,
    pub head_line: usize,
    pub head_col: usize,
}

impl BoxSel {
    pub fn lines(&self) -> (usize, usize) {
        (
            self.anchor_line.min(self.head_line),
            self.anchor_line.max(self.head_line),
        )
    }
    pub fn cols(&self) -> (usize, usize) {
        (
            self.anchor_col.min(self.head_col),
            self.anchor_col.max(self.head_col),
        )
    }
    /// Zero width: a column of carets (multi-line typing).
    pub fn is_carets(&self) -> bool {
        self.anchor_col == self.head_col
    }
}

/// One visual row of a (possibly soft-wrapped) line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seg {
    /// Char range of the line shown on this row.
    pub start: usize,
    pub end: usize,
    /// Display column within the whole line where this row starts.
    pub dstart: usize,
    /// Blank columns before the text (wrapped rows keep the line's indent).
    pub indent: usize,
}

/// Split a line into rows of at most `width` columns, breaking after
/// whitespace where possible (VS Code's "wordWrap" with "same" indent).
pub fn wrap_line(text: &str, width: usize, tab: usize) -> Vec<Seg> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut cols = Vec::with_capacity(n + 1);
    let mut d = 0;
    for &c in &chars {
        cols.push(d);
        d += char_width(c, d, tab);
    }
    cols.push(d);
    let width = width.max(4);
    let lead = chars
        .iter()
        .take_while(|c| **c == ' ' || **c == '\t')
        .count();
    let cont_indent = if cols[lead] <= width / 2 {
        cols[lead]
    } else {
        0
    };
    let mut segs = Vec::new();
    let mut s = 0;
    loop {
        let indent = if segs.is_empty() { 0 } else { cont_indent };
        let avail = width.saturating_sub(indent).max(1);
        let ds = cols[s];
        let mut e = s;
        while e < n && cols[e + 1] - ds <= avail {
            e += 1;
        }
        if e >= n {
            segs.push(Seg {
                start: s,
                end: n,
                dstart: ds,
                indent,
            });
            return segs;
        }
        if e == s {
            // A single char wider than the row.
            e = s + 1;
        } else if let Some(b) = (s + 1..=e)
            .rev()
            .find(|&j| {
                matches!(chars[j - 1], ' ' | '\t' | ',' | '(' | '[' | '{' | ';')
                    || (j < n && chars[j] == '.')
            })
            .filter(|&j| cols[j] - ds >= avail / 3)
        {
            e = b;
        }
        segs.push(Seg {
            start: s,
            end: e,
            dstart: ds,
            indent,
        });
        s = e;
    }
}

fn seg_index(segs: &[Seg], col: usize) -> usize {
    segs.iter().rposition(|s| s.start <= col).unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineChange {
    None,
    Added,
    Modified,
    /// One or more lines were deleted just above this line.
    DeletedAbove,
}

pub fn char_width(c: char, col: usize, tab: usize) -> usize {
    if c == '\t' {
        tab - (col % tab)
    } else {
        c.width()
            .unwrap_or(0)
            .max(if c.is_control() { 1 } else { 0 })
    }
}

/// Display column of a char column (tabs expanded, wide chars counted).
pub fn display_col(line: &str, char_col: usize, tab: usize) -> usize {
    let mut d = 0;
    for c in line.chars().take(char_col) {
        d += char_width(c, d, tab);
    }
    d
}

/// Char column closest to a display column.
pub fn char_col_at(line: &str, target: usize, tab: usize) -> usize {
    let mut d = 0;
    for (i, c) in line.chars().enumerate() {
        let w = char_width(c, d, tab);
        if d + w > target {
            return if target - d > w / 2 && w > 1 {
                i + 1
            } else {
                i
            };
        }
        d += w;
    }
    line.chars().count()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum CharClass {
    Space,
    Word,
    Punct,
    Newline,
}

fn class(c: char) -> CharClass {
    if c == '\n' {
        CharClass::Newline
    } else if c.is_whitespace() {
        CharClass::Space
    } else if is_word(c) {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

pub fn comment_token(syntax: &str) -> (&'static str, &'static str) {
    match syntax {
        "Python"
        | "Ruby"
        | "Bourne Again Shell (bash)"
        | "Shell-Unix-Generic"
        | "Perl"
        | "YAML"
        | "TOML"
        | "R"
        | "Makefile"
        | "Dockerfile"
        | "PowerShell"
        | "Elixir"
        | "Nim"
        | "CMake"
        | "Nix"
        | "Julia"
        | "Crystal"
        | "fish"
        | "GraphQL"
        | "Terraform"
        | "HCL"
        | "Git Ignore"
        | "Git Attributes"
        | "Git Config"
        | "Fish"
        | "Zsh"
        | "nginx"
        | "Apache Conf"
        | "Puppet"
        | "Ansible"
        | "Containerfile"
        | "Starlark" => ("#", ""),
        "SQL" | "Lua" | "Haskell" | "Elm" | "Ada" | "PureScript" | "VHDL" => ("--", ""),
        "Lisp" | "Clojure" | "Racket" | "Scheme" | "INI" | "Assembly (x86_64)" | "ARM Assembly" => {
            (";", "")
        }
        "Erlang" | "LaTeX" | "TeX" | "MATLAB" | "Octave" | "Prolog" => ("%", ""),
        "HTML" | "XML" | "Markdown" | "MultiMarkdown" | "Vue Component" | "Svelte" | "SVG" => {
            ("<!--", "-->")
        }
        "CSS" | "SCSS" | "Sass" | "LESS" => ("/*", "*/"),
        "Visual Basic" | "VB.NET" | "VBScript" => ("'", ""),
        "Batch File" => ("REM", ""),
        "Vim Script" | "VimL" => ("\"", ""),
        "OCaml" | "F#" | "Pascal" => ("(*", "*)"),
        _ => ("//", ""),
    }
}

impl Editor {
    pub fn new(doc: Document) -> Self {
        let first = doc.line_str(0);
        let syntax = highlight::detect_syntax(doc.path.as_deref(), &first);
        Self {
            doc,
            hl: HighlightCache::new(syntax),
            sel: Selection::default(),
            preferred_col: None,
            scroll_line: 0,
            scroll_col: 0,
            view_height: 30,
            view_width: 80,
            diff_cache: None,
            hl_pending: false,
            wrap_width: None,
            scroll_seg: 0,
            block: None,
        }
    }

    pub fn open(path: PathBuf) -> anyhow::Result<Self> {
        Ok(Self::new(Document::open(&path)?))
    }

    pub fn language(&self) -> &'static str {
        &self.hl.syntax().name
    }

    pub fn tab(&self) -> usize {
        self.doc.tab_width
    }

    /// (line, char col) of the caret.
    pub fn cursor(&self) -> (usize, usize) {
        self.doc.pos(self.sel.head)
    }

    /// Display column of the caret.
    pub fn cursor_display_col(&self) -> usize {
        let (l, c) = self.cursor();
        display_col(&self.doc.line_str(l), c, self.tab())
    }

    /// Propagate document edits to the highlighter.
    pub fn sync(&mut self) {
        if let Some(line) = self.doc.take_dirty_line() {
            self.hl.invalidate_from(line);
        }
    }

    pub fn selected_text(&self) -> String {
        self.doc
            .rope
            .slice(self.sel.start()..self.sel.end())
            .to_string()
    }

    // ------------------------------------------------------------ movement

    fn set_head(&mut self, head: usize, extend: bool) {
        let head = head.min(self.doc.rope.len_chars());
        self.sel = if extend {
            Selection::new(self.sel.anchor, head)
        } else {
            Selection::caret(head)
        };
    }

    pub fn move_to(&mut self, line: usize, col: usize, extend: bool) {
        let idx = self.doc.idx(line, col);
        self.set_head(idx, extend);
        self.preferred_col = None;
    }

    pub fn move_horizontal(&mut self, delta: isize, extend: bool) {
        self.preferred_col = None;
        if !extend && !self.sel.is_empty() {
            let to = if delta < 0 {
                self.sel.start()
            } else {
                self.sel.end()
            };
            self.sel = Selection::caret(to);
            return;
        }
        let len = self.doc.rope.len_chars() as isize;
        let head = (self.sel.head as isize + delta).clamp(0, len) as usize;
        self.set_head(head, extend);
    }

    pub fn move_vertical(&mut self, delta: isize, extend: bool) {
        let (line, col) = self.cursor();
        let segs = self.segments(line);
        let k = seg_index(&segs, col);
        let want = self
            .preferred_col
            .unwrap_or_else(|| self.visual_x(line, col).1);
        let (target, hit_edge) = self.step_rows((line, k), delta);
        if hit_edge {
            // Ran into the first/last row: go to the document edge.
            if delta < 0 {
                self.set_head(0, extend);
            } else if delta > 0 {
                self.set_head(self.doc.rope.len_chars(), extend);
            }
        } else {
            let tsegs = self.segments(target.0);
            let last = target.1 + 1 == tsegs.len();
            let c = self.col_at_x(target.0, &tsegs[target.1], last, want);
            let idx = self.doc.idx(target.0, c);
            self.set_head(idx, extend);
        }
        self.preferred_col = Some(want);
    }

    pub fn page(&mut self, down: bool, extend: bool) {
        let h = self.view_height.max(2) as isize - 1;
        self.move_vertical(if down { h } else { -h }, extend);
        self.scroll_by(if down { h } else { -h });
    }

    // ------------------------------------------------------------ soft wrap

    /// Visual rows of a line (a single row when word wrap is off).
    pub fn segments(&self, line: usize) -> Vec<Seg> {
        match self.wrap_width {
            Some(w) => wrap_line(&self.doc.line_str(line), w, self.tab()),
            None => vec![Seg {
                start: 0,
                end: self.doc.line_len(line),
                dstart: 0,
                indent: 0,
            }],
        }
    }

    /// (row within the line, visual x before horizontal scroll) of a char column.
    pub fn visual_x(&self, line: usize, col: usize) -> (usize, usize) {
        let segs = self.segments(line);
        let k = seg_index(&segs, col);
        let d = display_col(&self.doc.line_str(line), col, self.tab());
        (k, segs[k].indent + d - segs[k].dstart)
    }

    /// Char column on row `seg` closest to visual x.
    pub fn col_at_x(&self, line: usize, seg: &Seg, last: bool, x: usize) -> usize {
        let target = x.saturating_sub(seg.indent) + seg.dstart;
        let c = char_col_at(&self.doc.line_str(line), target, self.tab());
        let max = if last {
            seg.end
        } else {
            seg.end.saturating_sub(1).max(seg.start)
        };
        c.clamp(seg.start, max)
    }

    fn row_count(&self, line: usize) -> usize {
        if self.wrap_width.is_some() {
            self.segments(line).len()
        } else {
            1
        }
    }

    /// Move `delta` visual rows; also reports whether the document edge cut the move short.
    fn step_rows(&self, pos: (usize, usize), delta: isize) -> ((usize, usize), bool) {
        let n = delta.unsigned_abs();
        let t = if delta < 0 {
            self.row_back(pos, n)
        } else {
            self.row_fwd(pos, n)
        };
        let moved = if delta < 0 {
            self.rows_between(t, pos, n + 1)
        } else {
            self.rows_between(pos, t, n + 1)
        };
        (t, moved < n)
    }

    /// Step `n` visual rows back from (line, row).
    pub fn row_back(&self, (mut l, mut k): (usize, usize), n: usize) -> (usize, usize) {
        for _ in 0..n {
            if k > 0 {
                k -= 1;
            } else if l > 0 {
                l -= 1;
                k = self.row_count(l) - 1;
            } else {
                break;
            }
        }
        (l, k)
    }

    /// Step `n` visual rows forward from (line, row), stopping at the last row.
    pub fn row_fwd(&self, (mut l, mut k): (usize, usize), n: usize) -> (usize, usize) {
        let total = self.doc.len_lines();
        for _ in 0..n {
            if k + 1 < self.row_count(l) {
                k += 1;
            } else if l + 1 < total {
                l += 1;
                k = 0;
            } else {
                break;
            }
        }
        (l, k)
    }

    fn scroll_pos(&self) -> (usize, usize) {
        let l = self.scroll_line.min(self.doc.len_lines().saturating_sub(1));
        (l, self.scroll_seg.min(self.row_count(l) - 1))
    }

    /// The rows visible from the scroll position: (line, row index, segment, is last row of line).
    pub fn visual_rows(&self, h: usize) -> Vec<(usize, usize, Seg, bool)> {
        let total = self.doc.len_lines();
        let (mut l, mut k) = self.scroll_pos();
        let mut out = Vec::with_capacity(h);
        while out.len() < h && l < total {
            let segs = self.segments(l);
            while k < segs.len() && out.len() < h {
                out.push((l, k, segs[k], k + 1 == segs.len()));
                k += 1;
            }
            l += 1;
            k = 0;
        }
        out
    }

    /// Visual rows from `a` to `b` (capped at `cap`).
    fn rows_between(&self, a: (usize, usize), b: (usize, usize), cap: usize) -> usize {
        if b <= a {
            return 0;
        }
        if b.0 - a.0 > cap {
            return cap;
        }
        let mut n = self.row_count(a.0) - a.1;
        for l in a.0 + 1..b.0 {
            n += self.row_count(l);
            if n >= cap {
                return cap;
            }
        }
        if b.0 > a.0 { n + b.1 } else { b.1 - a.1 }
    }

    /// Visual Studio smart Home: toggle between first non-blank and column 0.
    pub fn home(&mut self, extend: bool) {
        let (line, col) = self.cursor();
        let text = self.doc.line_str(line);
        let first = text.chars().take_while(|c| c.is_whitespace()).count();
        let target = if col == first { 0 } else { first };
        self.move_to(line, target, extend);
    }

    pub fn end(&mut self, extend: bool) {
        let (line, _) = self.cursor();
        self.move_to(line, self.doc.line_len(line), extend);
    }

    pub fn doc_start(&mut self, extend: bool) {
        self.set_head(0, extend);
        self.preferred_col = None;
    }

    pub fn doc_end(&mut self, extend: bool) {
        self.set_head(self.doc.rope.len_chars(), extend);
        self.preferred_col = None;
    }

    fn word_boundary(&self, from: usize, forward: bool) -> usize {
        let rope = &self.doc.rope;
        let len = rope.len_chars();
        let mut i = from;
        if forward {
            if i >= len {
                return len;
            }
            if rope.char(i) == '\n' {
                return i + 1;
            }
            while i < len && class(rope.char(i)) == CharClass::Space {
                i += 1;
            }
            if i < len {
                let k = class(rope.char(i));
                if k == CharClass::Newline {
                    return i;
                }
                while i < len && class(rope.char(i)) == k {
                    i += 1;
                }
            }
            i
        } else {
            if i == 0 {
                return 0;
            }
            if rope.char(i - 1) == '\n' {
                return i - 1;
            }
            while i > 0 && class(rope.char(i - 1)) == CharClass::Space {
                i -= 1;
            }
            if i > 0 {
                let k = class(rope.char(i - 1));
                if k == CharClass::Newline {
                    return i;
                }
                while i > 0 && class(rope.char(i - 1)) == k {
                    i -= 1;
                }
            }
            i
        }
    }

    pub fn move_word(&mut self, forward: bool, extend: bool) {
        let to = self.word_boundary(self.sel.head, forward);
        self.set_head(to, extend);
        self.preferred_col = None;
    }

    /// Range of the word touching `idx`, if any.
    pub fn word_at(&self, idx: usize) -> Option<(usize, usize)> {
        let rope = &self.doc.rope;
        let len = rope.len_chars();
        let at_word = |i: usize| i < len && is_word(rope.char(i));
        let mut s = if at_word(idx) {
            idx
        } else if idx > 0 && at_word(idx - 1) {
            idx - 1
        } else {
            return None;
        };
        let mut e = s;
        while s > 0 && at_word(s - 1) {
            s -= 1;
        }
        while at_word(e) {
            e += 1;
        }
        Some((s, e))
    }

    pub fn select_all(&mut self) {
        self.sel = Selection::new(0, self.doc.rope.len_chars());
    }

    pub fn select_line(&mut self) {
        let (sl, _) = self.doc.pos(self.sel.start());
        let (el, _) = self.doc.pos(self.sel.end());
        let start = self.doc.line_start(sl);
        let end = if el + 1 < self.doc.len_lines() {
            self.doc.line_start(el + 1)
        } else {
            self.doc.rope.len_chars()
        };
        self.sel = Selection::new(start, end);
    }

    pub fn select_word_at(&mut self, idx: usize) {
        if let Some((s, e)) = self.word_at(idx) {
            self.sel = Selection::new(s, e);
        } else {
            self.sel = Selection::caret(idx);
        }
    }

    /// Ctrl+D: select the word under the caret, or jump to the next occurrence
    /// of the current selection (wrapping around).
    pub fn select_next_occurrence(&mut self) -> bool {
        if self.sel.is_empty() {
            if let Some((s, e)) = self.word_at(self.sel.head) {
                self.sel = Selection::new(s, e);
                return true;
            }
            return false;
        }
        let needle = self.selected_text();
        let hay = self.doc.rope.to_string();
        let start_byte = self.doc.rope.char_to_byte(self.sel.end());
        let found = hay[start_byte..]
            .find(&needle)
            .map(|b| b + start_byte)
            .or_else(|| hay.find(&needle));
        if let Some(b) = found {
            let s = self.doc.rope.byte_to_char(b);
            self.sel = Selection::new(s, s + needle.chars().count());
            true
        } else {
            false
        }
    }

    pub fn goto_line(&mut self, line: usize) {
        let line = line.min(self.doc.len_lines().saturating_sub(1));
        let text = self.doc.line_str(line);
        let first = text.chars().take_while(|c| c.is_whitespace()).count();
        self.move_to(line, first, false);
        self.center_on_cursor();
    }

    pub fn center_on_cursor(&mut self) {
        let (line, col) = self.cursor();
        let k = self.visual_x(line, col).0;
        (self.scroll_line, self.scroll_seg) = self.row_back((line, k), self.view_height / 2);
    }

    pub fn jump_to_bracket(&mut self) {
        let (line, col) = self.cursor();
        for c in [col, col.wrapping_sub(1)] {
            if let Some((l, cc)) = highlight::match_bracket(&self.hl, line, c) {
                self.move_to(l, cc, false);
                return;
            }
        }
    }

    /// Bracket pair touching the caret (for highlighting).
    pub fn bracket_pair(&self) -> Option<((usize, usize), (usize, usize))> {
        let (line, col) = self.cursor();
        for c in [col, col.wrapping_sub(1)] {
            if c == usize::MAX {
                continue;
            }
            if let Some(m) = highlight::match_bracket(&self.hl, line, c) {
                return Some(((line, c), m));
            }
        }
        None
    }

    // ------------------------------------------------------------ editing

    fn begin(&mut self, kind: EditKind) {
        self.doc.begin(kind, self.sel);
    }

    fn finish(&mut self) {
        self.doc.end(self.sel);
        self.preferred_col = None;
        self.sync();
    }

    fn delete_selection(&mut self) {
        if !self.sel.is_empty() {
            let (s, e) = (self.sel.start(), self.sel.end());
            self.doc.remove(s, e);
            self.sel = Selection::caret(s);
        }
    }

    pub fn insert_text(&mut self, text: &str) {
        if self.doc.is_read_only() {
            return;
        }
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        self.begin(EditKind::Other);
        self.delete_selection();
        let at = self.sel.head;
        self.doc.insert(at, &text);
        self.sel = Selection::caret(at + text.chars().count());
        self.finish();
    }

    /// Paste a whole-line clipboard entry above the caret line (VS behavior
    /// for text copied without a selection).
    pub fn insert_lines_above(&mut self, text: &str) {
        if self.doc.is_read_only() {
            return;
        }
        self.begin(EditKind::Other);
        let (line, col) = self.cursor();
        let at = self.doc.line_start(line);
        let mut t = text.to_string();
        if !t.ends_with('\n') {
            t.push('\n');
        }
        self.doc.insert(at, &t);
        let added = t.matches('\n').count();
        let idx = self.doc.idx(line + added, col);
        self.sel = Selection::caret(idx);
        self.finish();
    }

    pub fn type_char(&mut self, c: char) {
        if self.doc.is_read_only() {
            return;
        }
        let pairs = [
            ('(', ')'),
            ('[', ']'),
            ('{', '}'),
            ('"', '"'),
            ('\'', '\''),
            ('`', '`'),
        ];
        let next = self.doc.char_at(self.sel.head);
        let prev = if self.sel.head > 0 {
            self.doc.char_at(self.sel.head - 1)
        } else {
            None
        };

        // Wrap a selection in brackets/quotes.
        if !self.sel.is_empty()
            && let Some((o, cl)) = pairs.iter().find(|(o, _)| *o == c)
        {
            self.begin(EditKind::Other);
            let (s, e) = (self.sel.start(), self.sel.end());
            self.doc.insert(e, &cl.to_string());
            self.doc.insert(s, &o.to_string());
            self.sel = Selection::new(s + 1, e + 1);
            self.finish();
            return;
        }

        // Type over an auto-inserted closer.
        if self.sel.is_empty() && next == Some(c) && matches!(c, ')' | ']' | '}' | '"' | '\'' | '`')
        {
            self.move_horizontal(1, false);
            return;
        }

        self.begin(EditKind::Typing);
        self.delete_selection();
        let at = self.sel.head;

        // Closing brace on a blank line: snap to the enclosing block's indent.
        if c == '}' {
            let (line, col) = self.doc.pos(at);
            let text = self.doc.line_str(line);
            if text.chars().take(col).all(|ch| ch == ' ' || ch == '\t') && col > 0 {
                let unit = self.doc.indent.unit();
                let indent: String = text.chars().take(col).collect();
                if let Some(stripped) = indent.strip_suffix(&unit) {
                    let start = self.doc.line_start(line);
                    self.doc.remove(start, start + col);
                    self.doc.insert(start, stripped);
                    let at = start + stripped.chars().count();
                    self.doc.insert(at, "}");
                    self.sel = Selection::caret(at + 1);
                    self.finish();
                    return;
                }
            }
        }

        let auto_close = pairs
            .iter()
            .find(|(o, _)| *o == c)
            .map(|(_, cl)| *cl)
            .filter(|_| {
                let next_ok = next.is_none_or(|n| n.is_whitespace() || ")]};,".contains(n));
                let is_quote = matches!(c, '"' | '\'' | '`');
                let prev_ok = !is_quote || prev.is_none_or(|p| !is_word(p) && p != c);
                next_ok && prev_ok
            });
        let mut s = c.to_string();
        if let Some(cl) = auto_close {
            s.push(cl);
        }
        self.doc.insert(at, &s);
        self.sel = Selection::caret(at + 1);
        self.finish();
    }

    pub fn newline(&mut self) {
        if self.doc.is_read_only() {
            return;
        }
        self.begin(EditKind::Other);
        self.delete_selection();
        let at = self.sel.head;
        let (line, col) = self.doc.pos(at);
        let text = self.doc.line_str(line);
        let indent: String = text
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .take(col)
            .collect();
        let before = text.chars().take(col).collect::<String>();
        let before_trim = before.trim_end();
        let after = self.doc.char_at(at);
        let opens_block = before_trim.ends_with(['{', '(', '['])
            || (before_trim.ends_with(':') && matches!(self.language(), "Python" | "YAML"));
        let unit = self.doc.indent.unit();
        let mut ins = format!("\n{indent}");
        let mut caret_offset = ins.chars().count();
        if opens_block {
            ins.push_str(&unit);
            caret_offset += unit.chars().count();
            let closes = matches!(
                (before_trim.chars().last(), after),
                (Some('{'), Some('}')) | (Some('('), Some(')')) | (Some('['), Some(']'))
            );
            if closes {
                ins.push('\n');
                ins.push_str(&indent);
            }
        }
        // Drop trailing whitespace the caret leaves behind.
        let trailing = before.len() - before.trim_end_matches([' ', '\t']).len();
        let ws_chars = before[before.len() - trailing..].chars().count();
        let at = if ws_chars > 0 && ws_chars < col {
            self.doc.remove(at - ws_chars, at);
            at - ws_chars
        } else {
            at
        };
        self.doc.insert(at, &ins);
        self.sel = Selection::caret(at + caret_offset);
        self.finish();
    }

    pub fn backspace(&mut self) {
        if self.doc.is_read_only() {
            return;
        }
        if !self.sel.is_empty() {
            self.begin(EditKind::Other);
            self.delete_selection();
            self.finish();
            return;
        }
        let at = self.sel.head;
        if at == 0 {
            return;
        }
        self.begin(EditKind::Deleting);
        let (line, col) = self.doc.pos(at);
        let text = self.doc.line_str(line);
        let prev = self.doc.char_at(at - 1);
        let next = self.doc.char_at(at);
        let leading = text.chars().take(col).all(|c| c == ' ');
        if let (Indent::Spaces(n), true, true) = (self.doc.indent, leading, col > 0) {
            // Delete back to the previous indent stop.
            let remove = match col % n {
                0 => n,
                r => r,
            };
            self.doc.remove(at - remove, at);
            self.sel = Selection::caret(at - remove);
        } else if matches!(
            (prev, next),
            (Some('('), Some(')'))
                | (Some('['), Some(']'))
                | (Some('{'), Some('}'))
                | (Some('"'), Some('"'))
                | (Some('\''), Some('\''))
        ) {
            self.doc.remove(at - 1, at + 1);
            self.sel = Selection::caret(at - 1);
        } else {
            self.doc.remove(at - 1, at);
            self.sel = Selection::caret(at - 1);
        }
        self.finish();
    }

    pub fn delete_forward(&mut self) {
        if self.doc.is_read_only() {
            return;
        }
        self.begin(EditKind::Deleting);
        if self.sel.is_empty() {
            let at = self.sel.head;
            self.doc.remove(at, at + 1);
        } else {
            self.delete_selection();
        }
        self.finish();
    }

    pub fn delete_word(&mut self, forward: bool) {
        if self.doc.is_read_only() {
            return;
        }
        if !self.sel.is_empty() {
            self.backspace();
            return;
        }
        let to = self.word_boundary(self.sel.head, forward);
        self.begin(EditKind::Other);
        let (s, e) = if forward {
            (self.sel.head, to)
        } else {
            (to, self.sel.head)
        };
        self.doc.remove(s, e);
        self.sel = Selection::caret(s);
        self.finish();
    }

    fn selected_lines(&self) -> (usize, usize) {
        let (sl, _) = self.doc.pos(self.sel.start());
        let (el, ec) = self.doc.pos(self.sel.end());
        let el = if ec == 0 && el > sl { el - 1 } else { el };
        (sl, el)
    }

    /// Tab key: indent selected lines, or insert an indent unit at the caret.
    pub fn tab_key(&mut self) {
        if self.doc.is_read_only() {
            return;
        }
        let (sl, el) = self.selected_lines();
        if self.sel.is_empty() || sl == el && !self.sel_spans_line() {
            self.begin(EditKind::Typing);
            self.delete_selection();
            let at = self.sel.head;
            let ins = match self.doc.indent {
                Indent::Tabs => "\t".to_string(),
                Indent::Spaces(n) => {
                    let col = self.cursor_display_col();
                    " ".repeat(n - col % n)
                }
            };
            self.doc.insert(at, &ins);
            self.sel = Selection::caret(at + ins.chars().count());
            self.finish();
        } else {
            self.indent_lines(true);
        }
    }

    fn sel_spans_line(&self) -> bool {
        let (sl, sc) = self.doc.pos(self.sel.start());
        let (_, ec) = self.doc.pos(self.sel.end());
        sc == 0 && ec == self.doc.line_len(sl)
    }

    pub fn indent_lines(&mut self, indent: bool) {
        if self.doc.is_read_only() {
            return;
        }
        let (sl, el) = self.selected_lines();
        let unit = self.doc.indent.unit();
        let n = unit.chars().count();
        let (mut anchor, mut head) = (self.sel.anchor as isize, self.sel.head as isize);
        self.begin(EditKind::Other);
        for line in sl..=el {
            let start = self.doc.line_start(line);
            let shift = |p: &mut isize, by: isize| {
                if *p >= start as isize {
                    *p = (*p + by).max(start as isize);
                }
            };
            if indent {
                if self.doc.line_len(line) == 0 && sl != el {
                    continue;
                }
                self.doc.insert(start, &unit);
                shift(&mut anchor, n as isize);
                shift(&mut head, n as isize);
            } else {
                let text = self.doc.line_str(line);
                let remove = if text.starts_with('\t') {
                    1
                } else {
                    text.chars().take(n).take_while(|c| *c == ' ').count()
                };
                if remove > 0 {
                    self.doc.remove(start, start + remove);
                    shift(&mut anchor, -(remove as isize));
                    shift(&mut head, -(remove as isize));
                }
            }
        }
        self.sel = Selection::new(anchor.max(0) as usize, head.max(0) as usize);
        self.finish();
    }

    pub fn toggle_comment(&mut self) {
        if self.doc.is_read_only() {
            return;
        }
        let (open, close) = comment_token(self.language());
        let (sl, el) = self.selected_lines();
        let lines: Vec<String> = (sl..=el).map(|l| self.doc.line_str(l)).collect();
        let non_blank: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
        if non_blank.is_empty() {
            return;
        }
        let all_commented = non_blank.iter().all(|l| l.trim_start().starts_with(open));
        let min_indent = non_blank
            .iter()
            .map(|l| l.chars().take_while(|c| c.is_whitespace()).count())
            .min()
            .unwrap_or(0);
        self.begin(EditKind::Other);
        let before = self.sel;
        let mut delta_total: isize = 0;
        let mut first_delta: isize = 0;
        for (i, line) in (sl..=el).enumerate() {
            let text = &lines[i];
            if text.trim().is_empty() {
                continue;
            }
            let start = self.doc.line_start(line);
            let delta: isize;
            if all_commented {
                let ws = text.chars().take_while(|c| c.is_whitespace()).count();
                let rest: String = text.chars().skip(ws).collect();
                let mut remove = open.chars().count();
                if rest[open.len()..].starts_with(' ') {
                    remove += 1;
                }
                self.doc.remove(start + ws, start + ws + remove);
                delta = -(remove as isize);
                if !close.is_empty() {
                    let t = self.doc.line_str(line);
                    if let Some(p) = t.rfind(close) {
                        let mut s = t[..p].chars().count();
                        let mut len = close.chars().count();
                        if t[..p].ends_with(' ') {
                            s -= 1;
                            len += 1;
                        }
                        self.doc.remove(start + s, start + s + len);
                    }
                }
            } else {
                let ins = format!("{open} ");
                self.doc.insert(start + min_indent, &ins);
                delta = ins.chars().count() as isize;
                if !close.is_empty() {
                    let end = start + self.doc.line_len(line);
                    self.doc.insert(end, &format!(" {close}"));
                }
            }
            if i == 0 {
                first_delta = delta;
            }
            delta_total += delta;
        }
        let adj = |p: usize, d: isize| (p as isize + d).max(0) as usize;
        if before.is_empty() {
            self.sel = Selection::caret(adj(before.head, first_delta));
        } else {
            let (s, e) = (before.start(), before.end());
            let (ns, ne) = (adj(s, first_delta), adj(e, delta_total));
            self.sel = if before.head >= before.anchor {
                Selection::new(ns, ne)
            } else {
                Selection::new(ne, ns)
            };
        }
        self.finish();
    }

    /// Alt+Up/Down: move the selected lines.
    pub fn move_lines(&mut self, down: bool) {
        if self.doc.is_read_only() {
            return;
        }
        let (sl, el) = self.selected_lines();
        let last = self.doc.len_lines().saturating_sub(1);
        if (!down && sl == 0) || (down && el >= last) {
            return;
        }
        let (sa, sh) = (self.doc.pos(self.sel.anchor), self.doc.pos(self.sel.head));
        self.begin(EditKind::Other);
        let block: Vec<String> = (sl..=el).map(|l| self.doc.line_str(l)).collect();
        if down {
            let other = self.doc.line_str(el + 1);
            let start = self.doc.line_start(sl);
            let end = start + self.char_span(sl, el + 1);
            self.doc.remove(start, end);
            let mut text = other;
            for b in &block {
                text.push('\n');
                text.push_str(b);
            }
            self.doc.insert(start, &text);
        } else {
            let other = self.doc.line_str(sl - 1);
            let start = self.doc.line_start(sl - 1);
            let end = start + self.char_span(sl - 1, el);
            self.doc.remove(start, end);
            let mut text = block.join("\n");
            text.push('\n');
            text.push_str(&other);
            self.doc.insert(start, &text);
        }
        let d: isize = if down { 1 } else { -1 };
        let a = self.doc.idx((sa.0 as isize + d) as usize, sa.1);
        let h = self.doc.idx((sh.0 as isize + d) as usize, sh.1);
        self.sel = Selection::new(a, h);
        self.finish();
    }

    /// Number of chars from start of `from` line to end of `to` line (no trailing newline).
    fn char_span(&self, from: usize, to: usize) -> usize {
        self.doc.line_start(to) + self.doc.line_len(to) - self.doc.line_start(from)
    }

    pub fn duplicate_lines(&mut self, down: bool) {
        if self.doc.is_read_only() {
            return;
        }
        let (sl, el) = self.selected_lines();
        let block: Vec<String> = (sl..=el).map(|l| self.doc.line_str(l)).collect();
        let text = block.join("\n");
        self.begin(EditKind::Other);
        let end = self.doc.line_start(el) + self.doc.line_len(el);
        self.doc.insert(end, &format!("\n{text}"));
        if down {
            let n = (el - sl + 1) as isize;
            let (a, h) = (self.doc.pos(self.sel.anchor), self.doc.pos(self.sel.head));
            let ai = self.doc.idx((a.0 as isize + n) as usize, a.1);
            let hi = self.doc.idx((h.0 as isize + n) as usize, h.1);
            self.sel = Selection::new(ai, hi);
        }
        self.finish();
    }

    pub fn delete_lines(&mut self) {
        if self.doc.is_read_only() {
            return;
        }
        let (sl, el) = self.selected_lines();
        let (_, col) = self.cursor();
        self.begin(EditKind::Other);
        let start = self.doc.line_start(sl);
        let total = self.doc.len_lines();
        let (s, e) = if el + 1 < total {
            (start, self.doc.line_start(el + 1))
        } else if sl > 0 {
            (start - 1, self.doc.rope.len_chars())
        } else {
            (start, self.doc.rope.len_chars())
        };
        self.doc.remove(s, e);
        let line = sl.min(self.doc.len_lines().saturating_sub(1));
        self.sel = Selection::caret(self.doc.idx(line, col));
        self.finish();
    }

    pub fn insert_line(&mut self, below: bool) {
        if self.doc.is_read_only() {
            return;
        }
        let (line, _) = self.cursor();
        let text = self.doc.line_str(line);
        let indent: String = text
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        self.begin(EditKind::Other);
        if below {
            let end = self.doc.line_start(line) + self.doc.line_len(line);
            self.doc.insert(end, &format!("\n{indent}"));
            self.sel = Selection::caret(end + 1 + indent.chars().count());
        } else {
            let start = self.doc.line_start(line);
            self.doc.insert(start, &format!("{indent}\n"));
            self.sel = Selection::caret(start + indent.chars().count());
        }
        self.finish();
    }

    /// Returns (text, whole_line). Without a selection the whole line is copied.
    pub fn copy(&self) -> (String, bool) {
        if self.sel.is_empty() {
            let (line, _) = self.cursor();
            (format!("{}\n", self.doc.line_str(line)), true)
        } else {
            (self.selected_text(), false)
        }
    }

    pub fn cut(&mut self) -> (String, bool) {
        let r = self.copy();
        if self.sel.is_empty() {
            self.delete_lines();
        } else {
            self.begin(EditKind::Other);
            self.delete_selection();
            self.finish();
        }
        r
    }

    pub fn undo(&mut self) {
        self.block = None;
        if let Some(sel) = self.doc.undo() {
            self.sel = clamp_sel(sel, self.doc.rope.len_chars());
        }
        self.preferred_col = None;
        self.sync();
    }

    pub fn redo(&mut self) {
        self.block = None;
        if let Some(sel) = self.doc.redo() {
            self.sel = clamp_sel(sel, self.doc.rope.len_chars());
        }
        self.preferred_col = None;
        self.sync();
    }

    /// Replace the char range with text as a single undo step.
    pub fn replace_range(&mut self, start: usize, end: usize, text: &str) {
        if self.doc.is_read_only() {
            return;
        }
        self.begin(EditKind::Other);
        self.doc.remove(start, end);
        self.doc.insert(start, text);
        self.sel = Selection::caret(start + text.chars().count());
        self.finish();
    }

    /// Replace the whole buffer in one undo step, keeping the caret where it
    /// was as closely as possible. Used by Replace in Files.
    pub fn replace_document(&mut self, text: &str) {
        if self.doc.is_read_only() {
            return;
        }
        let head = self.sel.head;
        let old_len = self.doc.rope.len_chars();
        self.begin(EditKind::Other);
        self.doc.remove(0, old_len);
        self.doc.insert(0, text);
        self.sel = Selection::caret(head.min(self.doc.rope.len_chars()));
        self.finish();
    }

    /// Replace many ranges (sorted, non-overlapping) in one undo step.
    pub fn replace_all(&mut self, ranges: &[(usize, usize)], text: &str) {
        if self.doc.is_read_only() {
            return;
        }
        if ranges.is_empty() {
            return;
        }
        self.begin(EditKind::Other);
        for (s, e) in ranges.iter().rev() {
            self.doc.remove(*s, *e);
            self.doc.insert(*s, text);
        }
        self.sel = Selection::caret(self.sel.head.min(self.doc.rope.len_chars()));
        self.finish();
    }

    // ------------------------------------------------------------ box selection

    /// Grow or start a box selection by whole rows/columns (Alt+Shift+arrows).
    pub fn box_extend(&mut self, dl: isize, dc: isize) {
        let b = self.block.unwrap_or_else(|| {
            let (line, _) = self.cursor();
            let col = self.cursor_display_col();
            BoxSel {
                anchor_line: line,
                anchor_col: col,
                head_line: line,
                head_col: col,
            }
        });
        let last = self.doc.len_lines().saturating_sub(1) as isize;
        let line = (b.head_line as isize + dl).clamp(0, last) as usize;
        let col = (b.head_col as isize + dc).max(0) as usize;
        self.box_to(b.anchor_line, b.anchor_col, line, col);
    }

    /// Set the box explicitly (mouse/tap); also moves the caret to the head.
    pub fn box_to(&mut self, anchor_line: usize, anchor_col: usize, line: usize, col: usize) {
        self.block = Some(BoxSel {
            anchor_line,
            anchor_col,
            head_line: line,
            head_col: col,
        });
        let c = char_col_at(&self.doc.line_str(line), col, self.tab());
        self.sel = Selection::caret(self.doc.idx(line, c));
        self.preferred_col = Some(col);
    }

    /// Leave box mode, keeping the caret at the head.
    pub fn box_clear(&mut self) {
        self.block = None;
    }

    fn display_width(&self, line: usize) -> usize {
        let t = self.doc.line_str(line);
        display_col(&t, t.chars().count(), self.tab())
    }

    /// Char ranges covered on each line of the box.
    pub fn box_ranges(&self) -> Vec<(usize, usize, usize)> {
        let Some(b) = self.block else {
            return Vec::new();
        };
        let (top, bottom) = b.lines();
        let (left, right) = b.cols();
        (top..=bottom)
            .map(|l| {
                let t = self.doc.line_str(l);
                (
                    l,
                    char_col_at(&t, left, self.tab()),
                    char_col_at(&t, right, self.tab()),
                )
            })
            .collect()
    }

    pub fn box_text(&self) -> String {
        self.box_ranges()
            .iter()
            .map(|&(l, s, e)| {
                self.doc
                    .line_str(l)
                    .chars()
                    .skip(s)
                    .take(e - s)
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Replace the box on every line with `pieces[i]` (or `pieces[0]` everywhere).
    fn box_replace(&mut self, pieces: &[&str]) {
        if self.doc.is_read_only() {
            return;
        }
        let Some(b) = self.block else { return };
        let (top, bottom) = b.lines();
        let (left, right) = b.cols();
        self.doc.begin(EditKind::Typing, self.sel);
        let mut new_col = left;
        for (i, line) in (top..=bottom).enumerate() {
            let piece = pieces.get(i).or(pieces.first()).copied().unwrap_or("");
            let width = self.display_width(line);
            let start = self.doc.line_start(line);
            if width < left {
                if piece.is_empty() {
                    continue;
                }
                // Virtual space: pad short lines out to the box column.
                let pad = " ".repeat(left - width);
                self.doc
                    .insert(start + self.doc.line_len(line), &format!("{pad}{piece}"));
            } else {
                let t = self.doc.line_str(line);
                let (s, e) = (
                    char_col_at(&t, left, self.tab()),
                    char_col_at(&t, right, self.tab()),
                );
                self.doc.remove(start + s, start + e);
                self.doc.insert(start + s, piece);
            }
            if i == 0 {
                new_col = left + display_col(piece, piece.chars().count(), self.tab());
            }
        }
        self.block = Some(BoxSel {
            anchor_line: b.anchor_line,
            anchor_col: new_col,
            head_line: b.head_line,
            head_col: new_col,
        });
        let c = char_col_at(&self.doc.line_str(b.head_line), new_col, self.tab());
        self.sel = Selection::caret(self.doc.idx(b.head_line, c));
        self.doc.end(self.sel);
        self.preferred_col = Some(new_col);
        self.sync();
    }

    /// Type into every line of the box (column editing).
    pub fn box_type(&mut self, text: &str) {
        if self.doc.is_read_only() {
            return;
        }
        self.box_replace(&[text]);
    }

    /// Backspace: delete the box, or the char before each caret.
    pub fn box_backspace(&mut self) {
        if self.doc.is_read_only() {
            return;
        }
        let Some(b) = self.block else { return };
        if !b.is_carets() {
            self.box_replace(&[""]);
            return;
        }
        let col = b.anchor_col;
        if col == 0 {
            return;
        }
        // Widen the box one column to the left, then delete it.
        let prev = col - 1;
        self.block = Some(BoxSel {
            anchor_col: prev,
            ..b
        });
        self.box_replace(&[""]);
    }

    /// Delete: delete the box, or the char after each caret.
    pub fn box_delete(&mut self) {
        if self.doc.is_read_only() {
            return;
        }
        let Some(b) = self.block else { return };
        if b.is_carets() {
            self.block = Some(BoxSel {
                head_col: b.head_col + 1,
                ..b
            });
        }
        self.box_replace(&[""]);
    }

    /// Paste: one clipboard line per box line when the counts match.
    pub fn box_paste(&mut self, text: &str) {
        if self.doc.is_read_only() {
            return;
        }
        let Some(b) = self.block else { return };
        let (top, bottom) = b.lines();
        let text = text.strip_suffix('\n').unwrap_or(text);
        let pieces: Vec<&str> = text.split('\n').collect();
        if pieces.len() == bottom - top + 1 || pieces.len() == 1 {
            self.box_replace(&pieces);
        } else {
            self.block = None;
            self.insert_text(text);
        }
    }

    // ------------------------------------------------------------ external edits

    /// Replace the buffer with `new` (a file an agent just changed on disk)
    /// as a single undoable step. Only the changed middle is rewritten, so
    /// the caret and scroll position stay put where possible. The buffer is
    /// marked as matching disk, while the change bars keep showing what
    /// changed since you last saved.
    pub fn apply_external(&mut self, new: &str) {
        if self.doc.is_read_only() {
            return;
        }
        let new = new.replace("\r\n", "\n");
        let old = self.doc.rope.to_string();
        if old == new {
            return;
        }
        let a: Vec<char> = old.chars().collect();
        let b: Vec<char> = new.chars().collect();
        let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
        let max_suffix = a.len().min(b.len()) - prefix;
        let suffix = a
            .iter()
            .rev()
            .zip(b.iter().rev())
            .take(max_suffix)
            .take_while(|(x, y)| x == y)
            .count();
        let (old_end, new_end) = (a.len() - suffix, b.len() - suffix);
        let insert: String = b[prefix..new_end].iter().collect();
        let head = self.sel.head;
        self.doc.begin(EditKind::Other, self.sel);
        self.doc.remove(prefix, old_end);
        self.doc.insert(prefix, &insert);
        let delta = new_end as isize - old_end as isize;
        let moved = if head >= old_end {
            (head as isize + delta).max(0) as usize
        } else {
            head.min(prefix.max(head))
        };
        self.sel = Selection::caret(moved.min(self.doc.rope.len_chars()));
        self.block = None;
        self.doc.end(self.sel);
        self.doc.mark_saved();
        self.sync();
    }

    // ------------------------------------------------------------ viewport

    pub fn scroll_into_view(&mut self) {
        let (line, col) = self.cursor();
        let (k, x) = self.visual_x(line, col);
        let cur = (line, k);
        let h = self.view_height.max(1);
        let margin = 3.min(h / 3);
        let top = self.scroll_pos();
        if cur < top || self.rows_between(top, cur, h + 1) < margin {
            (self.scroll_line, self.scroll_seg) = self.row_back(cur, margin);
        } else if self.rows_between(top, cur, h + 1) + margin >= h {
            (self.scroll_line, self.scroll_seg) = self.row_back(cur, h.saturating_sub(margin + 1));
        }
        if self.wrap_width.is_some() {
            self.scroll_col = 0;
            return;
        }
        let w = self.view_width.max(1);
        let hmargin = 4.min(w / 4);
        if x < self.scroll_col + hmargin {
            self.scroll_col = x.saturating_sub(hmargin);
        } else if x + hmargin >= self.scroll_col + w {
            self.scroll_col = x + hmargin + 1 - w;
        }
    }

    /// Scroll by visual rows.
    pub fn scroll_by(&mut self, delta: isize) {
        let pos = self.scroll_pos();
        let n = delta.unsigned_abs();
        (self.scroll_line, self.scroll_seg) = if delta < 0 {
            self.row_back(pos, n)
        } else {
            self.row_fwd(pos, n)
        };
    }

    /// Jump the viewport so `line` is the top row (scrollbar/minimap drags).
    pub fn scroll_to_line(&mut self, line: usize) {
        self.scroll_line = line.min(self.doc.len_lines().saturating_sub(1));
        self.scroll_seg = 0;
    }

    /// Per-line change markers versus the saved file (like VS's change tracking margin).
    pub fn line_changes(&mut self) -> &[LineChange] {
        let version = self.doc.version;
        let stale = self.diff_cache.as_ref().is_none_or(|(v, _)| *v != version);
        if stale {
            let n = self.doc.len_lines();
            let mut marks = vec![LineChange::None; n];
            let current = self.doc.rope.to_string();
            if n < 50_000 && current != self.doc.saved_text {
                let diff = similar::TextDiff::configure()
                    .timeout(std::time::Duration::from_millis(50))
                    .diff_lines(&self.doc.saved_text, &current);
                for op in diff.ops() {
                    match *op {
                        similar::DiffOp::Insert {
                            new_index, new_len, ..
                        } => {
                            for m in marks.iter_mut().skip(new_index).take(new_len) {
                                *m = LineChange::Added;
                            }
                        }
                        similar::DiffOp::Replace {
                            new_index, new_len, ..
                        } => {
                            for m in marks.iter_mut().skip(new_index).take(new_len) {
                                *m = LineChange::Modified;
                            }
                        }
                        similar::DiffOp::Delete { new_index, .. } => {
                            if let Some(m) = marks.get_mut(new_index.min(n.saturating_sub(1)))
                                && *m == LineChange::None
                            {
                                *m = LineChange::DeletedAbove;
                            }
                        }
                        similar::DiffOp::Equal { .. } => {}
                    }
                }
            }
            self.diff_cache = Some((version, marks));
        }
        &self.diff_cache.as_ref().unwrap().1
    }
}

fn clamp_sel(s: Selection, len: usize) -> Selection {
    Selection::new(s.anchor.min(len), s.head.min(len))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(text: &str, ext: &str) -> Editor {
        Editor::new(Document::from_text(
            text,
            Some(PathBuf::from(format!("t.{ext}"))),
        ))
    }

    fn text(e: &Editor) -> String {
        e.doc.rope.to_string()
    }

    #[test]
    fn auto_close_and_type_over() {
        let mut e = ed("", "cs");
        e.type_char('(');
        assert_eq!(text(&e), "()");
        e.type_char('x');
        e.type_char(')');
        assert_eq!(text(&e), "(x)");
        assert_eq!(e.sel.head, 3);
    }

    #[test]
    fn enter_between_braces_expands_block() {
        let mut e = ed("    if (x) {}", "cs");
        e.move_to(0, 12, false);
        e.newline();
        assert_eq!(text(&e), "    if (x) {\n        \n    }");
        assert_eq!(e.cursor(), (1, 8));
    }

    #[test]
    fn backspace_removes_indent_stop_and_pairs() {
        let mut e = ed("        x", "cs");
        e.doc.indent = Indent::Spaces(4);
        e.move_to(0, 8, false);
        e.backspace();
        assert_eq!(text(&e), "    x");
        let mut e = ed("", "cs");
        e.type_char('[');
        e.backspace();
        assert_eq!(text(&e), "");
    }

    #[test]
    fn toggle_comment_roundtrip() {
        let src = "int a;\n  int b;\n";
        let mut e = ed(src, "cs");
        e.sel = Selection::new(0, e.doc.line_start(1) + 3);
        e.toggle_comment();
        // Like VS, the marker goes at the block's minimum indent.
        assert_eq!(text(&e), "// int a;\n//   int b;\n");
        e.toggle_comment();
        assert_eq!(text(&e), src);
        let mut p = ed("x = 1\n", "py");
        p.toggle_comment();
        assert_eq!(text(&p), "# x = 1\n");
    }

    #[test]
    fn move_and_duplicate_lines() {
        let mut e = ed("a\nb\nc", "txt");
        e.move_to(0, 0, false);
        e.move_lines(true);
        assert_eq!(text(&e), "b\na\nc");
        assert_eq!(e.cursor(), (1, 0));
        e.duplicate_lines(true);
        assert_eq!(text(&e), "b\na\na\nc");
        assert_eq!(e.cursor(), (2, 0));
        e.delete_lines();
        assert_eq!(text(&e), "b\na\nc");
    }

    #[test]
    fn word_movement_and_ctrl_d() {
        let mut e = ed("foo.bar baz foo", "txt");
        e.move_word(true, false);
        assert_eq!(e.sel.head, 3);
        e.move_to(0, 1, false);
        e.select_next_occurrence();
        assert_eq!(e.selected_text(), "foo");
        e.select_next_occurrence();
        assert_eq!(e.sel.start(), 12);
    }

    #[test]
    fn indent_and_outdent_selection() {
        let mut e = ed("a\nb", "txt");
        e.select_all();
        e.tab_key();
        assert_eq!(text(&e), "    a\n    b");
        e.indent_lines(false);
        assert_eq!(text(&e), "a\nb");
    }

    #[test]
    fn display_columns_expand_tabs() {
        assert_eq!(display_col("\tab", 1, 4), 4);
        assert_eq!(display_col("a\tb", 2, 4), 4);
        assert_eq!(char_col_at("\tab", 5, 4), 2);
        assert_eq!(display_col("日本", 1, 4), 2);
    }

    #[test]
    fn change_markers() {
        let mut e = ed("a\nb\nc\n", "txt");
        e.move_to(1, 1, false);
        e.type_char('!');
        assert_eq!(e.line_changes()[1], LineChange::Modified);
        assert_eq!(e.line_changes()[0], LineChange::None);
    }

    #[test]
    fn wrap_breaks_after_whitespace_and_keeps_indent() {
        let segs = wrap_line("    let total = order.lines.sum();", 24, 4);
        assert_eq!(segs.len(), 2);
        // Breaks before `.` so member chains wrap cleanly.
        assert_eq!(
            segs[0],
            Seg {
                start: 0,
                end: 21,
                dstart: 0,
                indent: 0
            }
        );
        assert_eq!(segs[1].start, 21);
        assert_eq!(segs[1].indent, 4);
        // Rows never exceed the width.
        for w in 5..40 {
            let text = "fn main() { let x = some_function_name(argument_one, argument_two); }";
            for seg in wrap_line(text, w, 4) {
                assert!(
                    seg.indent + display_col(text, seg.end, 4) - seg.dstart <= w.max(4)
                        || seg.end - seg.start == 1
                );
            }
        }
        assert_eq!(wrap_line("", 10, 4).len(), 1);
    }

    #[test]
    fn vertical_movement_follows_wrapped_rows() {
        let mut e = ed("aaaa bbbb cccc dddd\nx", "txt");
        e.wrap_width = Some(10);
        e.view_height = 3;
        assert_eq!(e.segments(0).len(), 2);
        e.move_to(0, 2, false);
        e.move_vertical(1, false);
        // Second row of line 0, same visual column.
        assert_eq!(e.cursor(), (0, 12));
        e.move_vertical(1, false);
        assert_eq!(e.cursor(), (1, 1));
        e.move_vertical(-2, false);
        assert_eq!(e.cursor(), (0, 2));
        e.move_vertical(-1, false);
        assert_eq!(e.sel.head, 0);
        // Scrolling counts rows, not lines.
        e.scroll_by(1);
        assert_eq!((e.scroll_line, e.scroll_seg), (0, 1));
        e.doc_end(false);
        e.scroll_into_view();
        let rows = e.visual_rows(3);
        assert!(rows.iter().any(|r| r.0 == 1));
    }

    #[test]
    fn box_selection_types_on_every_line_with_virtual_space() {
        let mut e = ed("abc\nx\nabcdef", "txt");
        e.move_to(0, 2, false);
        e.box_extend(2, 0);
        assert_eq!(e.block.unwrap().lines(), (0, 2));
        e.box_type("|");
        // Line 1 is shorter than the box column: padded with a space.
        assert_eq!(text(&e), "ab|c\nx |\nab|cdef");
        e.box_type("!");
        assert_eq!(text(&e), "ab|!c\nx |!\nab|!cdef");
        e.box_backspace();
        assert_eq!(text(&e), "ab|c\nx |\nab|cdef");
        e.undo();
        assert_eq!(text(&e), "abc\nx\nabcdef");
    }

    #[test]
    fn box_copy_delete_and_paste() {
        let mut e = ed("abcd\nefgh\nijkl", "txt");
        e.move_to(0, 1, false);
        e.box_extend(1, 2);
        assert_eq!(e.box_text(), "bc\nfg");
        e.box_delete();
        assert_eq!(text(&e), "ad\neh\nijkl");
        e.box_paste("12\n34\n");
        assert_eq!(text(&e), "a12d\ne34h\nijkl");
        assert!(e.block.unwrap().is_carets());
        e.box_delete();
        assert_eq!(text(&e), "a12\ne34\nijkl");
    }

    #[test]
    fn external_change_is_one_undo_step_and_keeps_change_bars() {
        let mut e = ed("def add(a, b):\n    return a - b\n", "py");
        e.move_to(0, 4, false);
        e.apply_external("def add(a, b):\n    return a + b\n");
        assert_eq!(text(&e), "def add(a, b):\n    return a + b\n");
        assert!(!e.doc.is_dirty(), "buffer matches disk");
        assert_eq!(e.cursor(), (0, 4), "caret before the change stays");
        assert_eq!(e.line_changes()[1], LineChange::Modified);
        e.undo();
        assert_eq!(text(&e), "def add(a, b):\n    return a - b\n");
        assert!(e.doc.is_dirty(), "undoing the agent's edit needs a save");
    }
}
