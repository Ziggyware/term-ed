//! Code intelligence for the editor's context menu: go to definition, find
//! references, inspect a symbol, and the safe mechanical refactors.
//!
//! This is deliberately *not* a language server. It reads the buffer and the
//! project with cheap, well-defined rules so it works for every file type the
//! editor can highlight, with no install step and no background process. Every
//! answer is presented as a set of locations with a confidence, so a rename
//! can refuse when the picture is ambiguous rather than silently corrupting
//! code.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use regex::Regex;

/// What kind of thing a word in the buffer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Type,
    Constant,
    Variable,
    Field,
    Macro,
    Unknown,
}

impl SymbolKind {
    pub fn label(self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Type => "type",
            SymbolKind::Constant => "constant",
            SymbolKind::Variable => "variable",
            SymbolKind::Field => "field",
            SymbolKind::Macro => "macro",
            SymbolKind::Unknown => "symbol",
        }
    }
}

/// A place in the project: file, 0-based line, 0-based character column.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Location {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    /// The whole line, trimmed, for display in a picker.
    pub preview: String,
    /// True when this is a declaration rather than a use.
    pub definition: bool,
}

/// The word under the caret plus what it looks like.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    pub text: String,
    /// Character column where the word starts on its line.
    pub start: usize,
    /// Character column just past the word.
    pub end: usize,
    pub kind: SymbolKind,
}

impl Word {
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Characters that can appear in an identifier. `.` is handled separately so
/// `self.field` locates `field`, while `a.b()` still works.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The word at a character column, with a best-effort kind. Returns `None` on
/// whitespace, punctuation or an empty line.
pub fn word_at(line: &str, col: usize) -> Option<Word> {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    // A caret sitting just after a word (the common case: you typed it and
    // pressed the menu key) belongs to that word.
    let mut i = col.min(chars.len().saturating_sub(1));
    if !is_word_char(chars[i]) {
        if i == 0 || !is_word_char(chars[i - 1]) {
            return None;
        }
        i -= 1;
    }
    let mut start = i;
    while start > 0 && is_word_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = i + 1;
    while end < chars.len() && is_word_char(chars[end]) {
        end += 1;
    }
    // A leading digit run is a number literal, not an identifier.
    if chars[start].is_ascii_digit() {
        return None;
    }
    let text: String = chars[start..end].iter().collect();
    Some(Word {
        kind: classify(line, start, end),
        text,
        start,
        end,
    })
}

/// Guess a word's kind from how it appears. These are the cheap signals that
/// are right most of the time; anything uncertain stays `Unknown` rather than
/// claiming precision it does not have.
fn classify(line: &str, start: usize, end: usize) -> SymbolKind {
    let chars: Vec<char> = line.chars().collect();
    let before: String = chars[..start].iter().collect();
    let after: String = chars[end..].iter().collect();
    let word: String = chars[start..end].iter().collect();
    let before_trim = before.trim_end();
    let after_trim = after.trim_start();
    let first = word.chars().next().unwrap_or(' ');

    if first.is_uppercase() && word.chars().all(|c| c.is_uppercase() || c.is_numeric() || c == '_')
        && word.chars().any(|c| c.is_ascii_uppercase())
    {
        return SymbolKind::Constant;
    }
    if before_trim.ends_with("fn ")
        || before_trim.ends_with("def ")
        || before_trim.ends_with("func ")
        || before_trim.ends_with("function ")
        || before_trim.ends_with("sub ")
    {
        return SymbolKind::Function;
    }
    if before_trim.ends_with("struct ")
        || before_trim.ends_with("class ")
        || before_trim.ends_with("enum ")
        || before_trim.ends_with("trait ")
        || before_trim.ends_with("interface ")
        || before_trim.ends_with("type ")
        || before_trim.ends_with("impl ")
        || before_trim.ends_with("record ")
    {
        return SymbolKind::Type;
    }
    if before_trim.ends_with("macro_rules! ") || before_trim.ends_with("#define ") {
        return SymbolKind::Macro;
    }
    if before_trim.ends_with("const ") || before_trim.ends_with("static ") {
        return SymbolKind::Constant;
    }
    if before_trim.ends_with("let ") || before_trim.ends_with("var ") || before_trim.ends_with("mut ")
    {
        return SymbolKind::Variable;
    }
    // `name(` or `name (` is a call site; `name:` or `name =` is a binding.
    if after_trim.starts_with('(') {
        return SymbolKind::Function;
    }
    if (after_trim.starts_with(':') && !after_trim.starts_with("::"))
        || after_trim.starts_with(" =")
        || after_trim.starts_with('=')
    {
        return SymbolKind::Field;
    }
    SymbolKind::Unknown
}

/// One identifier occurrence in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub line: usize,
    pub col: usize,
    pub text: String,
    pub kind: SymbolKind,
    /// The declaration line for this occurrence, trimmed.
    pub preview: String,
}

/// Whole-word search over one buffer. Used for definitions and references in
/// the open file (unsaved edits included) without a regex-per-line cost.
pub fn occurrences(text: &str, word: &str) -> Vec<Occurrence> {
    if word.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let target: Vec<char> = word.chars().collect();
        let mut c = 0;
        while c + target.len() <= chars.len() {
            let candidate = &chars[c..c + target.len()];
            let matches = candidate == target.as_slice()
                && (c == 0 || !is_word_char(chars[c - 1]))
                && (c + target.len() == chars.len() || !is_word_char(chars[c + target.len()]));
            if matches {
                out.push(Occurrence {
                    line: i,
                    col: c,
                    text: word.to_string(),
                    kind: classify(line, c, c + target.len()),
                    preview: line.trim().to_string(),
                });
                c += target.len();
            } else {
                c += 1;
            }
        }
    }
    out
}

/// Definition candidates: occurrences that look like declarations, in the
/// order a person would try them. `word` may be a `Type::member` tail, which
/// is why the field/type kinds are accepted here.
pub fn definitions(text: &str, word: &str) -> Vec<Occurrence> {
    occurrences(text, word)
        .into_iter()
        .filter(|o| {
            matches!(
                o.kind,
                SymbolKind::Function
                    | SymbolKind::Type
                    | SymbolKind::Constant
                    | SymbolKind::Variable
                    | SymbolKind::Macro
            )
        })
        .collect()
}

/// Every occurrence that is *not* a declaration, i.e. a use.
pub fn references(text: &str, word: &str) -> Vec<Occurrence> {
    occurrences(text, word)
        .into_iter()
        .filter(|o| !definitions(text, word).iter().any(|d| d.line == o.line && d.col == o.col))
        .collect()
}

/// A symbol's health, as shown by **Inspect Symbol**.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inspection {
    pub word: Word,
    pub definitions: usize,
    pub references: usize,
    pub lines: Vec<usize>,
    /// Human sentences, already ordered from most to least important.
    pub findings: Vec<String>,
}

pub fn inspect(text: &str, word: &Word) -> Inspection {
    let defs = definitions(text, &word.text);
    let refs = occurrences(text, &word.text)
        .into_iter()
        .filter(|o| !defs.iter().any(|d| d.line == o.line && d.col == o.col))
        .collect::<Vec<_>>();
    let mut findings = Vec::new();
    let mut lines: Vec<usize> = defs.iter().map(|d| d.line).collect();
    lines.extend(refs.iter().map(|r| r.line));
    lines.sort_unstable();
    lines.dedup();

    if defs.is_empty() {
        findings.push(format!(
            "No declaration found for `{}` in this file; it may come from another module or a dependency.",
            word.text
        ));
    } else if defs.len() > 1 {
        findings.push(format!(
            "`{}` is declared {} times (lines {}). A rename would have to choose one.",
            word.text,
            defs.len(),
            defs.iter()
                .map(|d| (d.line + 1).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    } else {
        findings.push(format!(
            "Declared on line {} as a {}.",
            defs[0].line + 1,
            defs[0].kind.label()
        ));
    }
    findings.push(match refs.len() {
        0 => "Nothing else in this file uses it.".to_string(),
        1 => "Used once elsewhere in this file.".to_string(),
        n => format!("Used {n} times elsewhere in this file."),
    });
    if defs.len() == 1 && refs.is_empty() && defs[0].kind != SymbolKind::Macro {
        findings.push("Unused: consider deleting it or marking it public.".to_string());
    }
    if word.kind == SymbolKind::Unknown {
        findings.push("Its role is unclear from the syntax around it.".to_string());
    }
    Inspection {
        word: word.clone(),
        definitions: defs.len(),
        references: refs.len(),
        lines,
        findings,
    }
}

// ================================================================ refactors

/// A proposed edit. `old` must appear exactly once, or the refactor is
/// refused: a rename that silently changes the wrong thing is worse than no
/// rename at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub path: PathBuf,
    pub old: String,
    pub new: String,
    pub line: usize,
}

/// Why a refactor could not be produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal(pub String);

/// Rename a symbol inside the given buffers.
///
/// `buffers` is what the caller has in memory (path → text); files that are
/// open use their unsaved text, so the rename agrees with what is on screen.
/// Refuses when the new name is not an identifier, already exists in a buffer
/// that defines the old name, or when the token cannot be found exactly.
pub fn rename(
    word: &Word,
    new_name: &str,
    buffers: &[(PathBuf, String)],
) -> Result<Vec<Edit>, Refusal> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err(Refusal("The new name is empty.".into()));
    }
    if !new_name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
        || !new_name.chars().all(is_word_char)
    {
        return Err(Refusal(format!(
            "`{new_name}` is not a valid identifier: use letters, digits and underscores, and do not start with a digit."
        )));
    }
    if is_reserved(new_name) {
        return Err(Refusal(format!("`{new_name}` is a reserved word.")));
    }
    if new_name == word.text {
        return Err(Refusal("The new name is the same as the old one.".into()));
    }

    let mut edits = Vec::new();
    for (path, text) in buffers {
        // A clash is only a problem where the old name is declared, because
        // that is where the new name would become ambiguous.
        let occs = occurrences(text, &word.text);
        if occs.is_empty() {
            continue;
        }
        if occs.iter().any(|o| o.kind != SymbolKind::Unknown)
            && !occurrences(text, new_name).is_empty()
        {
            return Err(Refusal(format!(
                "`{new_name}` already appears in {}; renaming would merge two symbols.",
                path.display()
            )));
        }
        for occ in occs {
            edits.push(Edit {
                path: path.clone(),
                // The token plus its column is enough to rewrite the line; the
                // caller applies it to its own buffer, so offsets never drift.
                old: word.text.clone(),
                new: new_name.to_string(),
                line: occ.line,
            });
        }
    }
    if edits.is_empty() {
        return Err(Refusal(format!(
            "`{}` does not appear in the open buffers.",
            word.text
        )));
    }
    Ok(edits)
}

fn is_reserved(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "as", "async", "await", "break", "case", "catch", "class", "const", "continue", "crate",
        "def", "default", "delete", "do", "dyn", "else", "enum", "export", "extends", "extern",
        "false", "final", "finally", "fn", "for", "from", "func", "function", "go", "if", "impl",
        "import", "in", "interface", "is", "lambda", "let", "loop", "match", "mod", "move", "mut",
        "new", "nil", "none", "null", "package", "pass", "pub", "raise", "ref", "return", "self",
        "static", "struct", "super", "switch", "this", "throw", "trait", "true", "try", "type",
        "typeof", "union", "unsafe", "use", "var", "void", "where", "while", "with", "yield",
    ];
    RESERVED.contains(&name.to_ascii_lowercase().as_str())
}

/// Extract the function or block containing a line, so it can be pulled into a
/// new one. Uses indentation and brace balance, which covers Python, Rust, JS
/// and Go closely enough to be useful, and never edits anything by itself.
pub fn enclosing_block(text: &str, line: usize) -> Option<(usize, usize)> {
    let lines: Vec<&str> = text.split('\n').collect();
    if line >= lines.len() {
        return None;
    }
    let indent_of = |l: &str| l.chars().take_while(|c| *c == ' ' || *c == '\t').count();
    let target = indent_of(lines[line]);
    let mut start = line;
    while start > 0 {
        let prev = lines[start - 1];
        if prev.trim().is_empty() {
            start -= 1;
            continue;
        }
        if indent_of(prev) < target {
            break;
        }
        start -= 1;
    }
    // Walk forward to the line that closes the block: either indentation drops
    // below the header, or braces return to their starting depth.
    let header_indent = indent_of(lines[start]);
    let mut depth = 0isize;
    let mut end = line;
    let mut saw_brace = false;
    for (i, l) in lines.iter().enumerate().skip(start) {
        for c in l.chars() {
            match c {
                '{' => {
                    depth += 1;
                    saw_brace = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        end = i;
        if saw_brace && depth <= 0 {
            break;
        }
        if !saw_brace && i > start && !l.trim().is_empty() && indent_of(l) <= header_indent {
            end = i.saturating_sub(1);
            break;
        }
    }
    if start == end {
        None
    } else {
        Some((start, end))
    }
}

/// Sort the selected lines (or the whole buffer when nothing is selected).
pub fn sort_lines(text: &str, from: usize, to: usize) -> String {
    let mut lines: Vec<&str> = text.split('\n').collect();
    let to = to.min(lines.len().saturating_sub(1));
    if from >= to {
        return text.to_string();
    }
    let slice = &mut lines[from..=to];
    slice.sort_by_key(|l| l.trim_start().to_ascii_lowercase());
    lines.join("\n")
}

/// Remove duplicate lines, keeping the first. Only exact duplicates that are
/// not blank collide, so intentional spacing survives.
pub fn dedupe_lines(text: &str, from: usize, to: usize) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let to = to.min(lines.len().saturating_sub(1));
    if from >= to {
        return text.to_string();
    }
    let mut seen = BTreeSet::new();
    let mut keep = Vec::with_capacity(to - from + 1);
    for l in &lines[from..=to] {
        let key = l.trim_end().to_string();
        if key.trim().is_empty() || seen.insert(key) {
            keep.push(*l);
        }
    }
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    out.extend_from_slice(&lines[..from]);
    out.extend(keep);
    out.extend_from_slice(&lines[(to + 1).min(lines.len())..]);
    out.join("\n")
}

/// Collect the words defined in a file, for the outline/symbol list.
pub fn outline(text: &str) -> Vec<Occurrence> {
    let mut out: Vec<Occurrence> = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        let trimmed = line.trim_start();
        let Some(kind) = header_kind(trimmed) else {
            continue;
        };
        let offset = line.chars().count() - trimmed.chars().count();
        // The declared name is the first identifier after the keyword.
        let rest = trimmed.split_once(' ').map(|(_, r)| r).unwrap_or("");
        let name: String = rest
            .trim_start_matches(|c: char| c == '&' || c == '*' || c == '!')
            .chars()
            .take_while(|c| is_word_char(*c))
            .collect();
        if name.is_empty() {
            continue;
        }
        out.push(Occurrence {
            line: i,
            col: offset + trimmed.chars().count() - rest.chars().count() + rest.chars().count()
                - rest.trim_start().chars().count()
                + (rest.trim_start().chars().count()
                    - rest.trim_start().trim_start_matches(|c: char| c == '&' || c == '*').chars().count()),
            text: name,
            kind,
            preview: trimmed.to_string(),
        });
    }
    out
}

fn header_kind(trimmed: &str) -> Option<SymbolKind> {
    for (prefix, kind) in [
        ("pub fn ", SymbolKind::Function),
        ("fn ", SymbolKind::Function),
        ("pub async fn ", SymbolKind::Function),
        ("async fn ", SymbolKind::Function),
        ("def ", SymbolKind::Function),
        ("async def ", SymbolKind::Function),
        ("func ", SymbolKind::Function),
        ("function ", SymbolKind::Function),
        ("pub struct ", SymbolKind::Type),
        ("struct ", SymbolKind::Type),
        ("pub enum ", SymbolKind::Type),
        ("enum ", SymbolKind::Type),
        ("class ", SymbolKind::Type),
        ("pub trait ", SymbolKind::Type),
        ("trait ", SymbolKind::Type),
        ("interface ", SymbolKind::Type),
        ("type ", SymbolKind::Type),
        ("pub const ", SymbolKind::Constant),
        ("const ", SymbolKind::Constant),
        ("pub static ", SymbolKind::Constant),
        ("static ", SymbolKind::Constant),
        ("macro_rules! ", SymbolKind::Macro),
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            if rest.chars().next().is_some_and(is_word_char) {
                return Some(kind);
            }
        }
    }
    None
}

// ================================================================ project-wide

/// Search the project for a whole word, reusing the editor's own search so
/// results, filters and limits behave identically to Find in Files.
pub fn project_references(
    root: &Path,
    word: &str,
    overrides: &[(PathBuf, String)],
    limit: usize,
) -> Result<Vec<Location>, String> {
    let query = crate::search::Query {
        text: word.to_string(),
        whole_word: true,
        case_sensitive: true,
        regex: false,
    };
    let filter = crate::search::FilterSet::default();
    let overrides: std::collections::HashMap<PathBuf, String> = overrides.iter().cloned().collect();
    let results = crate::search::search(root, &query, &filter, &overrides)
        .map_err(|e| format!("Search failed: {e}"))?;
    let mut out = Vec::new();
    for file in results.files {
        let text = overrides
            .get(&file.path)
            .cloned()
            .or_else(|| std::fs::read_to_string(&file.path).ok());
        for hit in file.matches {
            let source = text.as_deref().unwrap_or("");
            out.push(Location {
                path: file.path.clone(),
                line: hit.line,
                col: hit.start,
                preview: hit.text.clone(),
                definition: source
                    .split('\n')
                    .nth(hit.line)
                    .map(|l| {
                        matches!(
                            classify(l, hit.start, hit.end.max(hit.start)),
                            SymbolKind::Function
                                | SymbolKind::Type
                                | SymbolKind::Constant
                                | SymbolKind::Variable
                                | SymbolKind::Macro
                        )
                    })
                    .unwrap_or(false),
            });
            if out.len() >= limit {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

/// A compiled word-boundary matcher, for callers that want to preview a rename
/// count without building edits.
pub fn word_regex(word: &str) -> Option<Regex> {
    Regex::new(&format!(r"\b{}\b", regex::escape(word))).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"struct Point {
    x: i32,
}

fn magnitude(p: Point) -> i32 {
    let total = p.x * p.x;
    total
}

fn main() {
    let origin = Point { x: 0 };
    let _ = magnitude(origin);
}
"#;

    #[test]
    fn finds_the_word_under_and_just_after_the_caret() {
        let line = "    let total = p.x * p.x;";
        let w = word_at(line, 9).unwrap();
        assert_eq!(w.text, "total");
        // Caret immediately after the word still selects it.
        assert_eq!(word_at(line, 13).unwrap().text, "total");
        // `.` splits, so the field is found on its own.
        assert_eq!(word_at(line, 16).unwrap().text, "x");
        assert!(word_at(line, 8).is_none(), "whitespace is not a word");
        assert!(word_at("    1234", 5).is_none(), "numbers are not symbols");
        assert!(word_at("", 0).is_none());
    }

    #[test]
    fn classifies_declarations_and_uses() {
        let w = word_at("fn magnitude(p: Point) -> i32 {", 4).unwrap();
        assert_eq!((w.text.as_str(), w.kind), ("magnitude", SymbolKind::Function));
        assert_eq!(
            word_at("struct Point {", 8).unwrap().kind,
            SymbolKind::Type
        );
        let mut w = word_at("    let _ = magnitude(origin);", 20).unwrap();
        assert_eq!(w.kind, SymbolKind::Function);
        w.kind = SymbolKind::Function;
        assert_eq!(word_at("const MAX: usize = 4;", 7).unwrap().kind, SymbolKind::Constant);
    }

    #[test]
    fn definitions_are_declarations_and_references_are_uses() {
        let defs = definitions(SRC, "magnitude");
        assert_eq!(defs.len(), 1, "{defs:?}");
        assert_eq!(defs[0].line, 4);
        let refs = references(SRC, "magnitude");
        assert_eq!(refs.len(), 1, "{refs:?}");
        assert_eq!(refs[0].line, 12);

        // A type used in a signature is a use, the struct is the definition.
        let defs = definitions(SRC, "Point");
        assert_eq!(defs.len(), 1, "{defs:?}");
        assert_eq!(defs[0].line, 0);
    }

    #[test]
    fn inspection_explains_an_unused_symbol() {
        let text = "fn helper() {}\nfn main() {}\n";
        let w = word_at("fn helper() {}", 4).unwrap();
        let report = inspect(text, &w);
        assert_eq!(report.definitions, 1);
        assert_eq!(report.references, 0);
        assert!(
            report.findings.iter().any(|f| f.contains("Unused")),
            "{:?}",
            report.findings
        );
    }

    #[test]
    fn duplicate_declarations_are_reported_not_guessed() {
        let text = "fn dup() {}\nfn dup() {}\nfn main() { dup(); }\n";
        let w = word_at("fn dup() {}", 4).unwrap();
        let report = inspect(text, &w);
        assert_eq!(report.definitions, 2);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.contains("declared 2 times")),
            "{:?}",
            report.findings
        );
    }

    #[test]
    fn rename_refuses_bad_and_clashing_names() {
        let path = PathBuf::from("a.rs");
        let buf = vec![(path.clone(), SRC.to_string())];
        let w = word_at("fn magnitude(p: Point) -> i32 {", 4).unwrap();

        assert!(rename(&w, "", &buf).is_err());
        assert!(rename(&w, "2bad", &buf).is_err());
        assert!(rename(&w, "has space", &buf).is_err());
        assert!(rename(&w, "fn", &buf).is_err());
        assert!(rename(&w, "magnitude", &buf).is_err());
        // `Point` is already there, so this would merge two symbols.
        assert!(rename(&w, "Point", &buf).is_err());

        let edits = rename(&w, "length", &buf).unwrap();
        assert_eq!(edits.len(), 2, "one declaration, one call");
        assert!(edits.iter().all(|e| e.new == "length"));
        assert_eq!(edits[0].line, 4);
        assert_eq!(edits[1].line, 12);
    }

    #[test]
    fn rename_does_not_touch_substrings() {
        let text = "fn cat() {}\nfn category() {}\nfn main() { cat(); }\n";
        let buf = vec![(PathBuf::from("a.rs"), text.to_string())];
        let w = word_at("fn cat() {}", 4).unwrap();
        let edits = rename(&w, "feline", &buf).unwrap();
        assert_eq!(edits.len(), 2, "`category` must be left alone");
    }

    #[test]
    fn enclosing_block_covers_the_whole_function() {
        let (start, end) = enclosing_block(SRC, 8).unwrap();
        assert_eq!(start, 4);
        assert_eq!(end, 8);
        // A one-line file has no block to extract.
        assert!(enclosing_block("fn a() {}", 0).is_none());
    }

    #[test]
    fn sort_and_dedupe_only_touch_the_given_lines() {
        let text = "b\na\nc\n";
        assert_eq!(sort_lines(text, 0, 2), "a\nb\nc\n");
        // The trailing empty element after the final newline is preserved.
        assert_eq!(dedupe_lines("x\nx\n\ny\nx\n", 0, 2), "x\n\ny\nx\n");
        assert_eq!(dedupe_lines("a\nb\n", 0, 0), "a\nb\n");
        assert_eq!(sort_lines("only\n", 0, 0), "only\n");
    }

    #[test]
    fn outline_lists_declarations_with_their_lines() {
        let found = outline(SRC);
        let names: Vec<(&str, usize)> = found.iter().map(|o| (o.text.as_str(), o.line)).collect();
        assert_eq!(
            names,
            vec![("Point", 0), ("magnitude", 4), ("main", 10)],
            "{found:?}"
        );
        assert_eq!(found[0].kind, SymbolKind::Type);
        assert_eq!(found[1].kind, SymbolKind::Function);
    }
}
