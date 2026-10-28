//! Find and replace across the whole project.
//!
//! Files are chosen with a VS Code-style filter string: glob patterns
//! separated by `;`, `!` at the front to exclude, and a trailing `/` or `\`
//! to mean a directory (which takes all of its sub-folders with it). For
//! example `*.cs*;!*.csproj;!bin\` searches C# files but skips project files
//! and anything under a folder called `bin`.
//!
//! The walker reuses ripgrep's [`ignore`] crate, so `.gitignore` is honored,
//! and the search itself is line based, like the editor's find bar.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};

/// Larger files are skipped: they are rarely source, and searching them
/// would make every keystroke slow.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Most matches kept across the whole project.
pub const MAX_MATCHES: usize = 2_000;
/// Most matches kept for a single file.
pub const MAX_MATCHES_PER_FILE: usize = 200;
/// Most files read before stopping; protects against huge trees.
pub const MAX_FILES: usize = 4_000;

// ================================================================ query

/// What to look for, and how.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

impl Query {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Build the regex used for matching. Literal queries are escaped, so
    /// `$` and friends only mean something in regex mode.
    pub fn compile(&self) -> Result<Regex, regex::Error> {
        let mut pattern = if self.regex {
            self.text.clone()
        } else {
            regex::escape(&self.text)
        };
        if self.whole_word {
            pattern = format!(r"\b(?:{pattern})\b");
        }
        RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .build()
    }
}

// ================================================================ filters

/// One parsed entry of the filter string.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Glob {
    pattern: GlobPattern,
    /// A trailing `/` or `\` in the filter: the pattern names a directory.
    dir: bool,
}

impl Glob {
    /// Match the complete path and every path tail. Testing tails includes the
    /// file name, so a separate allocation and match for the name is unnecessary.
    fn matches(&self, rel: &str) -> bool {
        let text: Vec<char> = rel.chars().map(fold).collect();
        if self.pattern.matches_chars(&text) {
            return true;
        }
        text.iter()
            .enumerate()
            .any(|(i, c)| *c == '/' && self.pattern.matches_chars(&text[i + 1..]))
    }
}

/// A parsed filter: which directories to walk and which files to look at.
///
/// An empty include list means "everything", so an empty `FilterSet` searches
/// the whole root. Directory rules are the ones from the panel's `dirs` box
/// (`bin;src`) and from legacy trailing-separator entries in the file box.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilterSet {
    includes: Vec<Glob>,
    excludes: Vec<Glob>,
    /// Only these directories (and their sub-directories) are searched.
    dir_includes: Vec<Glob>,
    /// These directories and everything under them are skipped.
    dir_excludes: Vec<Glob>,
}

impl FilterSet {
    /// Parse one combined filter string, as Visual Studio's *file* box does:
    /// `*.cs*;!*.csproj;!bin\`. Entries with a trailing separator are
    /// directory rules; everything else applies to file names.
    pub fn parse(spec: &str) -> Self {
        let mut set = Self::default();
        for (exclude, g) in parse_entries(spec) {
            if g.dir {
                if exclude {
                    set.dir_excludes.push(g);
                } else {
                    set.dir_includes.push(g);
                }
            } else if exclude {
                set.excludes.push(g);
            } else {
                set.includes.push(g);
            }
        }
        set
    }

    /// Parse the panel's separate boxes: `dirs` (`bin;!obj;src`) and `files`
    /// (`*.cs;!*.csproj`). Every entry in `dirs` is a directory rule, whether
    /// or not it was written with a trailing separator, and `src` (without a
    /// `!`) scopes the search to that folder instead of skipping it.
    pub fn parse_parts(dirs: &str, files: &str) -> Self {
        let mut set = Self::default();
        for (exclude, mut g) in parse_entries(dirs) {
            g.dir = true;
            if exclude {
                set.dir_excludes.push(g);
            } else {
                set.dir_includes.push(g);
            }
        }
        for (exclude, g) in parse_entries(files) {
            if g.dir {
                // `!bin\` in the file box keeps working as a folder exclude.
                if exclude {
                    set.dir_excludes.push(g);
                } else {
                    set.dir_includes.push(g);
                }
            } else if exclude {
                set.excludes.push(g);
            } else {
                set.includes.push(g);
            }
        }
        set
    }

    pub fn is_empty(&self) -> bool {
        self.includes.is_empty()
            && self.excludes.is_empty()
            && self.dir_includes.is_empty()
            && self.dir_excludes.is_empty()
    }

    /// Does a root-relative file path pass the filter?
    pub fn matches_file(&self, rel: &str) -> bool {
        if !self.dir_includes.is_empty() && !self.dir_includes.iter().any(|g| in_dir(rel, g)) {
            return false;
        }
        if self.dir_excludes.iter().any(|g| hits_path(rel, g)) {
            return false;
        }
        if !self.includes.is_empty() {
            let included = self.includes.iter().any(|g| {
                if g.dir {
                    in_dir(rel, g)
                } else {
                    g.matches(rel)
                }
            });
            if !included {
                return false;
            }
        }
        !self.excludes.iter().any(|g| hits_path(rel, g))
    }

    /// May the walker descend into this directory? Directory rules prune (and
    /// scope) the tree; file patterns never do, so a broad include is not
    /// silently narrowed by an over-eager directory skip.
    pub fn allows_dir(&self, rel: &str) -> bool {
        if self.dir_excludes.iter().any(|g| hits_path(rel, g)) {
            return false;
        }
        // Includes match *any path tail*. A directory that does not match
        // yet can still contain a match (nested/src, src/**/tests, etc.).
        // Only exclusions are safe to prune; matches_file applies scopes.
        true
    }
}

/// Split a filter string into `(excluded, glob)` pairs. Empty entries are
/// ignored, `\` and `/` are interchangeable, and `!` may be followed by
/// whitespace.
fn parse_entries(spec: &str) -> Vec<(bool, Glob)> {
    let mut out = Vec::new();
    for part in spec.split([';', '\n']) {
        let mut t = part.trim();
        let exclude = t.starts_with('!');
        if exclude {
            t = t[1..].trim_start();
        }
        if t.is_empty() {
            continue;
        }
        let mut text = t.replace('\\', "/");
        while text.starts_with("./") {
            text.drain(..2);
        }
        let dir = text.ends_with('/');
        while text.ends_with('/') {
            text.pop();
        }
        if text.is_empty() {
            continue;
        }
        let pattern = GlobPattern::parse(&text);
        out.push((exclude, Glob { pattern, dir }));
    }
    out
}

/// `a/b/c.txt` -> `["a", "a/b"]`.
fn ancestors(rel: &str) -> impl Iterator<Item = &str> {
    rel.match_indices('/').map(move |(i, _)| &rel[..i])
}

/// A file is inside a directory pattern when any of its ancestors match.
fn in_dir(rel: &str, g: &Glob) -> bool {
    ancestors(rel).any(|d| g.matches(d))
}

/// Exclusions are a little more generous than includes: a pattern that names
/// a directory takes its whole subtree with it, whether or not it was written
/// with a trailing separator (`!node_modules` behaves like `!node_modules/`).
fn hits_path(rel: &str, g: &Glob) -> bool {
    g.matches(rel) || ancestors(rel).any(|d| g.matches(d))
}

/// Case-insensitive matching on Windows, exact elsewhere.
fn fold(c: char) -> char {
    if cfg!(windows) {
        c.to_ascii_lowercase()
    } else {
        c
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ClassMember {
    Literal(char),
    Range(char, char),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum GlobToken {
    Literal(char),
    Any,
    Star,
    DoubleStar,
    /// `**/` matches either nothing or any prefix ending in `/`.
    DoubleStarSlash,
    Class {
        negate: bool,
        members: Vec<ClassMember>,
    },
}

impl GlobToken {
    fn is_variable(&self) -> bool {
        matches!(
            self,
            Self::Star | Self::DoubleStar | Self::DoubleStarSlash
        )
    }

    fn matches_one(&self, c: char) -> bool {
        match self {
            Self::Literal(want) => c == *want,
            Self::Any => c != '/',
            Self::Class { negate, members } => {
                c != '/'
                    && members.iter().any(|member| match member {
                        ClassMember::Literal(want) => c == *want,
                        ClassMember::Range(start, end) => c >= *start && c <= *end,
                    }) != *negate
            }
            Self::Star | Self::DoubleStar | Self::DoubleStarSlash => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GlobPattern {
    tokens: Vec<GlobToken>,
}

impl GlobPattern {
    fn parse(pattern: &str) -> Self {
        let chars: Vec<char> = pattern.chars().map(fold).collect();
        let mut tokens = Vec::with_capacity(chars.len());
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '*' => {
                    let start = i;
                    while i < chars.len() && chars[i] == '*' {
                        i += 1;
                    }
                    if i - start == 1 {
                        tokens.push(GlobToken::Star);
                    } else if chars.get(i) == Some(&'/') {
                        tokens.push(GlobToken::DoubleStarSlash);
                        i += 1;
                    } else {
                        tokens.push(GlobToken::DoubleStar);
                    }
                }
                '?' => {
                    tokens.push(GlobToken::Any);
                    i += 1;
                }
                '[' => {
                    if let Some((class, consumed)) = parse_class(&chars[i..]) {
                        tokens.push(class);
                        i += consumed;
                    } else {
                        tokens.push(GlobToken::Literal('['));
                        i += 1;
                    }
                }
                c => {
                    tokens.push(GlobToken::Literal(c));
                    i += 1;
                }
            }
        }
        Self { tokens }
    }

    fn matches(&self, text: &str) -> bool {
        let text: Vec<char> = text.chars().map(fold).collect();
        self.matches_chars(&text)
    }

    /// Rolling NFA simulation: O(pattern × text) time, O(text) space, no
    /// recursion, and a hard state budget for adversarial filter strings.
    fn matches_chars(&self, text: &[char]) -> bool {
        const MAX_STATES: usize = 200_000;

        let Some(first) = self.tokens.iter().position(GlobToken::is_variable) else {
            return text.len() == self.tokens.len()
                && self
                    .tokens
                    .iter()
                    .zip(text)
                    .all(|(token, c)| token.matches_one(*c));
        };
        let last = self
            .tokens
            .iter()
            .rposition(GlobToken::is_variable)
            .unwrap_or(first);
        let suffix = self.tokens.len() - last - 1;
        if first + suffix > text.len()
            || !self.tokens[..first]
                .iter()
                .zip(text)
                .all(|(token, c)| token.matches_one(*c))
            || !self.tokens[last + 1..]
                .iter()
                .zip(&text[text.len() - suffix..])
                .all(|(token, c)| token.matches_one(*c))
        {
            return false;
        }
        let tokens = &self.tokens[first..=last];
        let text = &text[first..text.len() - suffix];
        let width = text.len() + 1;
        let mut current = vec![false; width];
        let mut next = vec![false; width];
        let mut budget = MAX_STATES;
        current[0] = true;

        for token in tokens {
            if width > budget {
                return false;
            }
            budget -= width;
            next.fill(false);
            match token {
                GlobToken::Literal(want) => {
                    for j in 1..width {
                        next[j] = current[j - 1] && text[j - 1] == *want;
                    }
                }
                GlobToken::Any => {
                    for j in 1..width {
                        next[j] = current[j - 1] && text[j - 1] != '/';
                    }
                }
                GlobToken::Class { negate, members } => {
                    for j in 1..width {
                        let c = text[j - 1];
                        next[j] = current[j - 1]
                            && c != '/'
                            && {
                                let matched = members.iter().any(|member| match member {
                                    ClassMember::Literal(want) => c == *want,
                                    ClassMember::Range(start, end) => c >= *start && c <= *end,
                                });
                                matched != *negate
                            };
                    }
                }
                GlobToken::Star => {
                    next[0] = current[0];
                    for j in 1..width {
                        next[j] = current[j] || (text[j - 1] != '/' && next[j - 1]);
                    }
                }
                GlobToken::DoubleStar => {
                    let mut reachable = false;
                    for j in 0..width {
                        reachable |= current[j];
                        next[j] = reachable;
                    }
                }
                GlobToken::DoubleStarSlash => {
                    next.copy_from_slice(&current);
                    let mut reachable = false;
                    for j in 1..width {
                        reachable |= current[j - 1];
                        if text[j - 1] == '/' && reachable {
                            next[j] = true;
                        }
                    }
                }
            }
            std::mem::swap(&mut current, &mut next);
        }
        current[text.len()]
    }
}

/// Parse a `[...]` class at the front of `pattern`, returning its token and
/// consumed character count. An unterminated class is handled literally.
fn parse_class(pattern: &[char]) -> Option<(GlobToken, usize)> {
    let mut i = 1;
    let negate = matches!(pattern.get(i), Some('!' | '^'));
    if negate {
        i += 1;
    }
    let mut members = Vec::new();
    let mut first = true;
    while i < pattern.len() {
        if pattern[i] == ']' && !first {
            return Some((GlobToken::Class { negate, members }, i + 1));
        }
        first = false;
        if i + 2 < pattern.len() && pattern[i + 1] == '-' && pattern[i + 2] != ']' {
            members.push(ClassMember::Range(pattern[i], pattern[i + 2]));
            i += 3;
        } else {
            members.push(ClassMember::Literal(pattern[i]));
            i += 1;
        }
    }
    None
}

/// Match one glob against one path fragment.
///
/// `*` and `?` never cross a `/`, `**` does, and `[abc]` / `[a-z]` / `[!a-z]`
/// character classes are supported.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    GlobPattern::parse(pattern).matches(text)
}

// ================================================================ files

/// A text file read from disk, with its line ending remembered so a replace
/// writes it back exactly as it was.
#[derive(Clone, Debug)]
pub struct TextFile {
    pub path: PathBuf,
    /// Always LF-normalized.
    pub text: String,
    pub crlf: bool,
}

impl TextFile {
    pub fn read(path: &Path) -> Option<Self> {
        Self::read_checked(path).ok().flatten()
    }

    pub fn read_checked(path: &Path) -> std::io::Result<Option<Self>> {
        if crate::credentials::is_private_path(path) {
            return Ok(None);
        }
        let meta = std::fs::metadata(path)?;
        if meta.len() > MAX_FILE_BYTES {
            return Ok(None);
        }
        let bytes = std::fs::read(path)?;
        if looks_binary(&bytes) {
            return Ok(None);
        }
        let Ok(mut text) = String::from_utf8(bytes) else {
            return Ok(None);
        };
        let crlf = text.contains("\r\n");
        if crlf {
            text = text.replace("\r\n", "\n");
        }
        Ok(Some(Self {
            path: path.to_path_buf(),
            text,
            crlf,
        }))
    }

    pub fn write(&self, text: &str) -> std::io::Result<()> {
        if self.crlf && !text.contains("\r\n") {
            std::fs::write(&self.path, text.replace('\n', "\r\n"))
        } else {
            std::fs::write(&self.path, text)
        }
    }
}

/// A NUL byte near the start almost always means binary.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8 * 1024).any(|b| *b == 0)
}

/// Project-relative path, with `/` separators on every platform.
pub fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Turn what the user typed in the panel's root box into a directory.
///
/// Empty means the project root, `~` means home, and relative paths are
/// resolved against the project root. `..` and `.` are folded lexically so
/// the label stays readable and the search key stays stable.
pub fn resolve_root(project: &Path, input: &str) -> PathBuf {
    let t = input.trim();
    if t.is_empty() || t == "." {
        return absolute_path(project);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let expanded = if t == "~" {
        home.clone().unwrap_or_else(|| PathBuf::from(t))
    } else if let Some(rest) = t.strip_prefix("~/").or_else(|| t.strip_prefix("~\\")) {
        home.map(|h| h.join(rest))
            .unwrap_or_else(|| PathBuf::from(t))
    } else {
        PathBuf::from(t)
    };
    let joined = if expanded.is_absolute() {
        expanded
    } else {
        project.join(expanded)
    };
    absolute_path(&joined)
}

/// Stable absolute identity for roots and open buffers. Canonicalize existing
/// paths (including symlinked project roots), but keep new unsaved paths usable.
pub fn absolute_path(p: &Path) -> PathBuf {
    let p = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let p = normalize(&p);
    p.canonicalize().unwrap_or(p)
}

/// Fold `.` and `..` without walking above an absolute filesystem root.
fn normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// A short, readable label for a search root (`~/code/app`, `/srv/app`).
pub fn root_label(root: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && let Ok(rest) = root.strip_prefix(&home)
    {
        let rest = rest.to_string_lossy().replace('\\', "/");
        return if rest.is_empty() {
            "~".to_string()
        } else {
            format!("~/{rest}")
        };
    }
    root.to_string_lossy().replace('\\', "/")
}

// ================================================================ search

/// One occurrence on one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// 0-based line number.
    pub line: usize,
    /// Char columns within the line.
    pub start: usize,
    pub end: usize,
    /// The full line, without its newline.
    pub text: String,
}

/// Matches in one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHits {
    pub path: PathBuf,
    /// Relative path with `/` separators, for display and filtering.
    pub rel: String,
    pub matches: Vec<Hit>,
}

/// Everything a search found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Results {
    pub files: Vec<FileHits>,
    pub matches: usize,
    /// Files read (after filtering).
    pub scanned: usize,
    /// A cap was hit; there may be more matches.
    pub truncated: bool,
    /// Up to three filesystem errors, rather than silently reporting no hits.
    pub warnings: Vec<String>,
}

impl Results {
    pub fn is_empty(&self) -> bool {
        self.matches == 0
    }
}

/// Search the project. `overrides` holds the in-memory text of open editors,
/// so unsaved edits are searched too.
pub fn search(
    root: &Path,
    query: &Query,
    filter: &FilterSet,
    overrides: &HashMap<PathBuf, String>,
) -> Result<Results, regex::Error> {
    let mut out = Results::default();
    if query.is_empty() {
        return Ok(out);
    }
    let re = query.compile()?;

    let root = absolute_path(root);
    let overrides: HashMap<PathBuf, &String> = overrides
        .iter()
        .map(|(p, text)| (absolute_path(p), text))
        .collect();
    let walk_root = root.clone();
    let walk_filter = filter.clone();
    let mut builder = WalkBuilder::new(&root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .filter_entry(move |e| {
            if e.file_name() == ".git" || crate::credentials::is_private_path(e.path()) {
                return false;
            }
            if e.depth() > 0 && e.file_type().is_some_and(|t| t.is_dir()) {
                return walk_filter.allows_dir(&relative(&walk_root, e.path()));
            }
            true
        });

    // Search open buffers first, even if they are new, ignored or no longer
    // on disk. They must not be lost behind the disk-walk file limit.
    let mut open: Vec<_> = overrides.keys().collect();
    open.sort();
    for path in open {
        if !path.starts_with(&root) || crate::credentials::is_private_path(path) {
            continue;
        }
        let rel = relative(&root, path);
        if rel.split('/').any(|part| part == ".git") || !filter.matches_file(&rel) {
            continue;
        }
        let text = overrides[path];
        if text.len() as u64 > MAX_FILE_BYTES {
            continue;
        }
        collect_hits(&mut out, path, rel, text, &re);
        if out.matches >= MAX_MATCHES {
            break;
        }
    }
    if out.matches < MAX_MATCHES {
        for entry in builder.build() {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    if out.warnings.len() < 3 {
                        out.warnings.push(e.to_string());
                    }
                    continue;
                }
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.path();
            if overrides.contains_key(&absolute_path(path)) {
                continue;
            }
            let rel = relative(&root, path);
            if !filter.matches_file(&rel) {
                continue;
            }
            if out.scanned >= MAX_FILES {
                out.truncated = true;
                break;
            }
            let file = match TextFile::read_checked(path) {
                Ok(Some(file)) => file,
                Ok(None) => continue,
                Err(e) => {
                    if out.warnings.len() < 3 {
                        out.warnings.push(format!("{}: {e}", path.display()));
                    }
                    continue;
                }
            };
            collect_hits(&mut out, path, rel, &file.text, &re);
            if out.matches >= MAX_MATCHES {
                break;
            }
        }
    }
    out.files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

fn collect_hits(out: &mut Results, path: &Path, rel: String, text: &str, re: &Regex) {
    out.scanned += 1;
    let mut hits = Vec::new();
    'lines: for (line_no, raw) in text.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let mut scanned_byte = 0;
        let mut scanned_char = 0;
        for m in re.find_iter(line).filter(|m| m.start() != m.end()) {
            if out.matches >= MAX_MATCHES || hits.len() >= MAX_MATCHES_PER_FILE {
                out.truncated = true;
                break 'lines;
            }
            scanned_char += line[scanned_byte..m.start()].chars().count();
            let start = scanned_char;
            let matched_chars = line[m.start()..m.end()].chars().count();
            let end = start + matched_chars;
            scanned_char = end;
            scanned_byte = m.end();
            hits.push(Hit {
                line: line_no,
                start,
                end,
                text: line.to_string(),
            });
            out.matches += 1;
        }
    }
    if !hits.is_empty() {
        out.files.push(FileHits {
            path: path.to_path_buf(),
            rel,
            matches: hits,
        });
    }
    if out.matches >= MAX_MATCHES {
        out.truncated = true;
    }
}

// ================================================================ replace

/// Replace every occurrence in an LF-normalized text. Returns the new text
/// and how many occurrences changed. `literal` keeps `$1` in the replacement
/// literal even though a regex was used to find matches (case-insensitive or
/// whole-word literal searches).
pub fn replace_in_text(
    text: &str,
    re: &Regex,
    replacement: &str,
    literal: bool,
) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut count = 0;
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let (new, n) = replace_in_line(line, re, replacement, literal);
        count += n;
        out.push_str(new.as_ref());
    }
    (out, count)
}

fn replace_in_line<'a>(
    line: &'a str,
    re: &Regex,
    replacement: &str,
    literal: bool,
) -> (Cow<'a, str>, usize) {
    let mut out = String::new();
    let mut last = 0;
    let mut count = 0;
    for caps in re.captures_iter(line) {
        let Some(m) = caps.get(0) else { continue };
        // Empty matches are never listed in the results, so they are not
        // replaced either (otherwise `x*` would insert text everywhere).
        if m.start() == m.end() {
            continue;
        }
        if count == 0 {
            out.reserve(line.len());
        }
        out.push_str(&line[last..m.start()]);
        if literal {
            out.push_str(replacement);
        } else {
            caps.expand(replacement, &mut out);
        }
        last = m.end();
        count += 1;
    }
    if count == 0 {
        return (Cow::Borrowed(line), 0);
    }
    out.push_str(&line[last..]);
    (Cow::Owned(out), count)
}

/// Replace only the match that starts at `start_char` on `line`. Used by the
/// per-result replace button. Returns `None` when the line no longer has a
/// match there (for example because the file changed since the search).
pub fn replace_at(
    line: &str,
    start_char: usize,
    re: &Regex,
    replacement: &str,
    literal: bool,
) -> Option<String> {
    let start_byte = line
        .char_indices()
        .map(|(byte, _)| byte)
        .chain(std::iter::once(line.len()))
        .nth(start_char)?;
    let m = re.find_at(line, start_byte)?;
    if m.start() != start_byte || m.start() == m.end() {
        return None;
    }
    let mut out = String::with_capacity(line.len().saturating_add(replacement.len()));
    out.push_str(&line[..m.start()]);
    if literal {
        out.push_str(replacement);
    } else {
        let caps = re.captures_at(line, m.start())?;
        caps.expand(replacement, &mut out);
    }
    out.push_str(&line[m.end()..]);
    Some(out)
}

// ================================================================ tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_example_from_the_spec() {
        let f = FilterSet::parse(r"*.cs*;!*.csproj;!bin\");
        assert!(f.matches_file("Program.cs"));
        assert!(f.matches_file("src/deep/Widget.cs"));
        assert!(f.matches_file("src/Generated.cs.txt"));
        assert!(!f.matches_file("src/App.csproj"));
        assert!(!f.matches_file("src/bin/Debug/Gen.cs"));
        assert!(!f.matches_file("src/bin/Debug/nested/Deep.cs"));
        assert!(!f.matches_file("bin/Tool.cs"));
        assert!(!f.matches_file("README.md"));
    }

    #[test]
    fn empty_filter_matches_everything() {
        let f = FilterSet::parse("  ;;  !");
        assert!(f.is_empty());
        assert!(f.matches_file("anything/at/all.txt"));
    }

    #[test]
    fn directory_includes_use_slashes_either_way() {
        let f = FilterSet::parse(r"src/");
        assert!(f.matches_file("src/main.rs"));
        assert!(f.matches_file("src/a/b/main.rs"));
        assert!(!f.matches_file("tests/main.rs"));
        let f = FilterSet::parse(r"src\");
        assert!(f.matches_file("src/main.rs"));
    }

    #[test]
    fn globs_respect_separators_and_double_stars() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(!glob_match("*.rs", "src/main.rs"));
        assert!(glob_match("src/*.rs", "src/main.rs"));
        assert!(!glob_match("src/*.rs", "src/a/main.rs"));
        assert!(glob_match("src/**/*.rs", "src/a/main.rs"));
        assert!(glob_match("src/**/*.rs", "src/main.rs"));
        assert!(glob_match("**/test?", "a/b/test1"));
        assert!(glob_match("[_a-z]*.md", "_notes.md"));
        assert!(!glob_match("[!_a-z]*.md", "_notes.md"));
        assert!(!glob_match("[!_a-z]*.md", "notes.md"));
        assert!(glob_match("[!_a-z]*.md", "9notes.md"));
    }

    #[test]
    fn glob_double_star_slash_and_malformed_classes_are_total() {
        assert!(glob_match("src/**/main.rs", "src/main.rs"));
        assert!(glob_match("src/**/main.rs", "src/a/b/main.rs"));
        assert!(!glob_match("src/**/main.rs", "src/a/b/main.rs.bak"));
        assert!(glob_match("**/*.rs", "main.rs"));
        assert!(glob_match("é?", "é界"));
        assert!(!glob_match("?", "/"));
        assert!(glob_match("[]]", "]"));
        assert!(glob_match("[abc", "[abc"));
    }

    #[test]
    fn long_literal_globs_bypass_the_wildcard_budget() {
        let literal = "a".repeat(1_000);
        assert!(glob_match(&literal, &literal));
        assert!(glob_match(&format!("*{literal}"), &literal));
    }

    #[test]
    fn adversarial_globs_stop_within_the_state_budget() {
        let pattern = "*a".repeat(1_000);
        assert!(!glob_match(&pattern, &"a".repeat(999)));
    }

    #[test]
    fn hit_columns_advance_once_through_unicode_lines() {
        let mut out = Results::default();
        let re = Regex::new("é").unwrap();
        collect_hits(&mut out, Path::new("unicode.txt"), "unicode.txt".into(), "aé éé", &re);
        let columns: Vec<(usize, usize)> = out.files[0]
            .matches
            .iter()
            .map(|hit| (hit.start, hit.end))
            .collect();
        assert_eq!(columns, vec![(1, 2), (3, 4), (4, 5)]);
    }

    #[test]
    fn exclusions_apply_at_any_depth() {
        let f = FilterSet::parse("!build/");
        assert!(!f.matches_file("build/out.js"));
        assert!(!f.matches_file("a/build/out.js"));
        assert!(!f.matches_file("a/build/deep/out.js"));
        assert!(f.matches_file("src/builder.rs"));
    }

    #[test]
    fn replace_expands_groups_in_regex_mode_and_not_otherwise() {
        let re = Regex::new(r"(\w+),(\w+)").unwrap();
        let (text, n) = replace_in_text("a,b\nc,d\n", &re, "$2 $1", false);
        assert_eq!(n, 2);
        assert_eq!(text, "b a\nd c\n");
        let re = Regex::new("x").unwrap();
        let (text, n) = replace_in_text("x\n", &re, "$1", true);
        assert_eq!(n, 1);
        assert_eq!(text, "$1\n");
    }

    #[test]
    fn replace_at_targets_one_occurrence() {
        let re = Regex::new("foo").unwrap();
        assert_eq!(
            replace_at("foo foo foo", 4, &re, "bar", true).unwrap(),
            "foo bar foo"
        );
        assert!(replace_at("foo", 2, &re, "bar", true).is_none());
        assert_eq!(
            replace_at("é foo foo", 6, &re, "bar", true).unwrap(),
            "é foo bar"
        );
        assert!(replace_at("foo", usize::MAX, &re, "bar", true).is_none());
    }

    #[test]
    fn separate_directory_and_file_filters() {
        // Directory box: `!bin` skips that folder at any depth.
        let f = FilterSet::parse_parts("!bin;!obj", "*.cs;!*.csproj");
        assert!(f.matches_file("src/Program.cs"));
        assert!(f.matches_file("deep/nested/Widget.cs"));
        assert!(!f.matches_file("bin/Tool.cs"));
        assert!(!f.matches_file("src/bin/Debug/Gen.cs"));
        assert!(!f.matches_file("src/obj/Gen.cs"));
        assert!(!f.matches_file("src/App.csproj"));
        assert!(!f.matches_file("README.md"));

        // A plain entry scopes the walk instead of excluding it.
        let f = FilterSet::parse_parts("src", "*");
        assert!(f.matches_file("src/a/b/main.rs"));
        assert!(!f.matches_file("tests/main.rs"));
        assert!(f.allows_dir("src"));
        assert!(f.allows_dir("src/a"));
        // A nonmatching folder can contain a nested src/ scope.
        assert!(f.allows_dir("tests"));
        // On the way to a scope, the walker may descend.
        let f = FilterSet::parse_parts("src/app", "*");
        assert!(f.allows_dir("src"));
        assert!(f.allows_dir("tests"));

        // `!` in the directory box excludes; globs work there too.
        let f = FilterSet::parse_parts("!**/generated", "*.rs");
        assert!(!f.matches_file("a/generated/x.rs"));
        assert!(f.matches_file("a/src/x.rs"));

        // The file box still accepts a legacy trailing-separator directory rule.
        let f = FilterSet::parse_parts("", r"*.cs*;!*.csproj;!bin\");
        assert!(!f.matches_file("src/bin/Debug/Gen.cs"));
        assert!(f.matches_file("src/Program.cs"));
    }

    #[test]
    fn roots_resolve_relative_to_the_project() {
        let project = Path::new("/work/app");
        assert_eq!(resolve_root(project, ""), PathBuf::from("/work/app"));
        assert_eq!(resolve_root(project, "  "), PathBuf::from("/work/app"));
        assert_eq!(resolve_root(project, "."), PathBuf::from("/work/app"));
        assert_eq!(resolve_root(project, "src"), PathBuf::from("/work/app/src"));
        assert_eq!(
            resolve_root(project, "src/../tests"),
            PathBuf::from("/work/app/tests")
        );
        assert_eq!(
            resolve_root(project, "/srv/other"),
            PathBuf::from("/srv/other")
        );
    }
}
