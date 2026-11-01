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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};

pub const DEFAULT_RESULT_LIMIT: usize = 10_000;
pub const MAX_RESULT_LIMIT: usize = 100_000;
/// Per-worker input bound, not a project-wide file or result limit.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Replacement has a separate conservative per-file bound.
pub const MAX_REPLACE_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PREVIEW_CHARS: usize = 2_048;

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
            .size_limit(2 * 1024 * 1024)
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
        if meta.len() > MAX_REPLACE_FILE_BYTES {
            return Ok(None);
        }
        let bytes = std::fs::read(path)?;
        if memchr::memchr(0, &bytes).is_some() {
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

/// Independent search targets. Content replacement never renames files or folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchOptions {
    pub content: bool,
    pub file_names: bool,
    pub directory_names: bool,
    pub ignore_binary: bool,
    pub max_results: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self { content: true, file_names: false, directory_names: false,
            ignore_binary: true, max_results: DEFAULT_RESULT_LIMIT }
    }
}

impl SearchOptions {
    pub fn any_target(&self) -> bool {
        self.content || self.file_names || self.directory_names
    }
    pub fn limit(&self) -> usize {
        self.max_results.clamp(1, MAX_RESULT_LIMIT)
    }
    /// One extra probe distinguishes "exactly at the limit" from "capped".
    fn probe_limit(&self) -> usize { self.limit().saturating_add(1) }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MatchKind {
    #[default]
    Content,
    FileName,
    DirectoryName,
}

/// One occurrence on one line, or one name hit (line zero).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub line: usize,
    /// Character columns, relative to `text` (not a truncated source line).
    pub start: usize,
    pub end: usize,
    pub text: String,
    /// Character offset of this preview in the original line.
    pub text_offset: usize,
    pub kind: MatchKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHits {
    pub path: PathBuf,
    pub rel: String,
    pub matches: Vec<Hit>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub visited: usize,
    pub scanned: usize,
    pub skipped_binary: usize,
    pub skipped_large: usize,
    pub current: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Results {
    pub files: Vec<FileHits>,
    pub matches: usize,
    pub scanned: usize,
    pub visited: usize,
    pub skipped_binary: usize,
    pub skipped_large: usize,
    /// The user-requested result budget was reached; no hidden per-file cap.
    pub truncated: bool,
    pub warnings: Vec<String>,
}

impl Results {
    pub fn is_empty(&self) -> bool { self.matches == 0 }
    pub fn progress(&self) -> Progress {
        Progress { visited: self.visited, scanned: self.scanned,
            skipped_binary: self.skipped_binary, skipped_large: self.skipped_large,
            current: String::new() }
    }
    /// Merge a worker batch in path order without rescanning every hit.
    fn merge(&mut self, mut files: Vec<FileHits>, limit: usize) {
        // Incoming batches are sorted once; existing files are already ordered.
        // Only files that receive new hits are re-sorted, not the whole list.
        for f in &mut files { f.matches.sort_by_key(|m| (m.kind as u8, m.line, m.start)); }
        files.sort_by(|a,b| a.rel.cmp(&b.rel));
        let mut merged = Vec::with_capacity(self.files.len() + files.len());
        let old = std::mem::take(&mut self.files);
        let mut a = old.into_iter().peekable();
        let mut b = files.into_iter().peekable();
        while a.peek().is_some() || b.peek().is_some() {
            let next = match (a.peek(), b.peek()) {
                (Some(x), Some(y)) if x.rel <= y.rel => a.next().unwrap(),
                (_, Some(_)) => b.next().unwrap(),
                _ => a.next().unwrap(),
            };
            if merged.last().is_some_and(|last: &FileHits| last.path == next.path) {
                let last = merged.last_mut().unwrap();
                last.matches.extend(next.matches);
                last.matches.sort_by_key(|m| (m.kind as u8, m.line, m.start));
            } else { merged.push(next); }
        }
        self.files = merged;
        let total: usize = self.files.iter().map(|f| f.matches.len()).sum();
        self.truncated |= total > limit;
        self.matches = 0;
        for file in &mut self.files {
            let take = limit.saturating_sub(self.matches).min(file.matches.len());
            file.matches.truncate(take);
            self.matches += take;
        }
        self.files.retain(|f| !f.matches.is_empty());
    }
}

#[derive(Default)]
struct Counters {
    cancel: AtomicBool,
    matches: AtomicUsize,
    visited: AtomicUsize,
    scanned: AtomicUsize,
    skipped_binary: AtomicUsize,
    skipped_large: AtomicUsize,
    warnings: AtomicUsize,
}

enum Update {
    Batch(Vec<FileHits>),
    Current(String),
    Warning(String),
    Error(String),
    Done,
}

/// Cancellation is cooperative and lock-free. Dropping a job also cancels it;
/// the UI never joins a filesystem worker or waits for a worker mutex.
pub struct SearchJob {
    rx: Receiver<Update>,
    counters: Arc<Counters>,
    pub options: SearchOptions,
    pub progress: Progress,
    pub running: bool,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    started: Instant,
}

impl SearchJob {
    pub fn cancel(&self) { self.counters.cancel.store(true, Ordering::Relaxed); }
    pub fn elapsed(&self) -> Duration { self.started.elapsed() }
    pub fn is_cancelled(&self) -> bool { self.counters.cancel.load(Ordering::Relaxed) }

    /// Bounded, nonblocking inbox drain. Call once per UI tick.
    pub fn pump(&mut self, out: &mut Results) -> bool {
        let before = (out.matches, out.files.len(), self.progress.visited,
            self.progress.scanned, self.progress.skipped_binary, self.progress.skipped_large,
            self.progress.current.clone(), self.warnings.len(), self.running, self.error.is_some());
        let mut merged_hits = 0usize;
        for _ in 0..256 {
            let msg = match self.rx.try_recv() {
                Ok(msg) => msg,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.running = false;
                    break;
                }
            };
            match msg {
                Update::Batch(files) => {
                    merged_hits += files.iter().map(|f| f.matches.len()).sum::<usize>();
                    out.merge(files, self.options.limit());
                    if out.truncated { self.cancel(); }
                    // Keep a single UI tick bounded even when many worker
                    // batches arrive together; the next tick drains the rest.
                    if merged_hits >= 8192 { break; }
                }
                Update::Current(path) => self.progress.current = path,
                Update::Warning(w) => { if self.warnings.len() < 3 { self.warnings.push(w); } }
                Update::Error(e) => { self.error = Some(e); self.cancel(); }
                Update::Done => self.running = false,
            }
        }
        self.progress.visited = self.counters.visited.load(Ordering::Relaxed);
        self.progress.scanned = self.counters.scanned.load(Ordering::Relaxed);
        self.progress.skipped_binary = self.counters.skipped_binary.load(Ordering::Relaxed);
        self.progress.skipped_large = self.counters.skipped_large.load(Ordering::Relaxed);
        out.visited = self.progress.visited;
        out.scanned = self.progress.scanned;
        out.skipped_binary = self.progress.skipped_binary;
        out.skipped_large = self.progress.skipped_large;
        out.warnings.clone_from(&self.warnings);
        if !self.running { self.progress.current.clear(); }
        before != (out.matches, out.files.len(), self.progress.visited,
            self.progress.scanned, self.progress.skipped_binary, self.progress.skipped_large,
            self.progress.current.clone(), self.warnings.len(), self.running, self.error.is_some())
    }
}

impl Drop for SearchJob {
    fn drop(&mut self) { self.cancel(); }
}

pub fn search_async(root: &Path, query: &Query, filter: &FilterSet,
    overrides: &HashMap<PathBuf, String>, options: SearchOptions) -> Result<SearchJob, regex::Error> {
    let re = query.compile()?;
    let root = absolute_path(root);
    let overrides: Arc<HashMap<PathBuf, String>> = Arc::new(overrides.iter()
        .map(|(p,t)| (absolute_path(p), t.clone())).collect());
    let filter = filter.clone();
    let counters = Arc::new(Counters::default());
    let (tx, rx) = mpsc::channel();
    let job_counters = counters.clone();
    if !root.is_dir() {
        let _ = tx.send(Update::Error(format!("Folder does not exist: {}", root.display())));
        let _ = tx.send(Update::Done);
    } else if query.is_empty() || !options.any_target() {
        let _ = tx.send(Update::Done);
    } else {
        let spawned = std::thread::Builder::new().name("termed-search".into()).spawn(move || {
            run_search(root, re, filter, overrides, options, counters, tx);
        });
        if let Err(e) = spawned {
            let _ = job_counters.cancel.swap(true, Ordering::Relaxed);
            return Err(regex::Error::Syntax(format!("Cannot start search: {e}")));
        }
    }
    Ok(SearchJob { rx, counters: job_counters, options, progress: Progress::default(),
        running: true, error: None, warnings: Vec::new(), started: Instant::now() })
}

/// Synchronous API for tools/tests. Interactive callers use `search_async`.
pub fn search(root: &Path, query: &Query, filter: &FilterSet,
    overrides: &HashMap<PathBuf, String>) -> Result<Results, regex::Error> {
    search_with_options(root, query, filter, overrides, &SearchOptions::default())
}

pub fn search_with_options(root: &Path, query: &Query, filter: &FilterSet,
    overrides: &HashMap<PathBuf, String>, options: &SearchOptions) -> Result<Results, regex::Error> {
    let mut job = search_async(root, query, filter, overrides, *options)?;
    let mut out = Results::default();
    while job.running {
        job.pump(&mut out);
        if job.running { std::thread::sleep(Duration::from_millis(1)); }
    }
    job.pump(&mut out);
    if let Some(error) = &job.error { out.warnings.push(error.clone()); }
    Ok(out)
}

struct SearchContext {
    root: PathBuf,
    re: Regex,
    filter: FilterSet,
    overrides: Arc<HashMap<PathBuf, String>>,
    options: SearchOptions,
    counters: Arc<Counters>,
    tx: Sender<Update>,
}

impl SearchContext {
    fn stopped(&self) -> bool {
        self.counters.cancel.load(Ordering::Relaxed)
            || self.counters.matches.load(Ordering::Relaxed) >= self.options.probe_limit()
    }
    fn send_files(&self, files: &mut Vec<FileHits>) {
        if files.is_empty() { return; }
        let n: usize = files.iter().map(|f| f.matches.len()).sum();
        self.counters.matches.fetch_add(n, Ordering::Relaxed);
        let _ = self.tx.send(Update::Batch(std::mem::take(files)));
    }
    fn warn(&self, text: String) {
        if self.counters.warnings.fetch_add(1, Ordering::Relaxed) < 3 {
            let _ = self.tx.send(Update::Warning(text));
        }
    }
    fn name_hit(&self, path: &Path, kind: MatchKind) -> Option<FileHits> {
        let name = path.file_name()?.to_string_lossy();
        let m = self.re.find(&name).filter(|m| !m.is_empty())?;
        Some(FileHits { path: path.to_path_buf(), rel: relative(&self.root, path),
            matches: vec![Hit { line: 0, start: name[..m.start()].chars().count(),
                end: name[..m.end()].chars().count(), text: name.into_owned(),
                text_offset: 0, kind }] })
    }
    fn visit(&self, entry: &ignore::DirEntry, batch: &mut Vec<FileHits>) {
        if self.stopped() { return; }
        self.counters.visited.fetch_add(1, Ordering::Relaxed);
        let path = entry.path();
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        let is_file = entry.file_type().is_some_and(|t| t.is_file());
        if is_file && self.overrides.contains_key(&absolute_path(path)) { return; }
        if entry.depth() > 0 && is_dir && self.options.directory_names
            && let Some(hit) = self.name_hit(path, MatchKind::DirectoryName) { batch.push(hit); }
        if !is_file { self.send_files(batch); return; }
        let rel = relative(&self.root, path);
        if !self.filter.matches_file(&rel) { self.send_files(batch); return; }
        let _ = self.tx.send(Update::Current(rel.clone()));
        let mut local = Vec::new();
        if self.options.file_names && let Some(hit) = self.name_hit(path, MatchKind::FileName) {
            local.push(hit);
        }
        let live = self.overrides.get(&absolute_path(path));
        if self.options.content {
            self.counters.scanned.fetch_add(1, Ordering::Relaxed);
            let text = if let Some(text) = live {
                if self.options.ignore_binary && text.as_bytes().contains(&0) {
                    self.counters.skipped_binary.fetch_add(1, Ordering::Relaxed);
                    None
                } else { Some(Cow::Borrowed(text.as_str())) }
            } else {
                match read_search_file(path, self.options.ignore_binary, &self.counters) {
                    Ok(Some(text)) => Some(Cow::Owned(text)),
                    Ok(None) => None,
                    Err(e) => { self.warn(format!("{}: {e}", path.display())); None }
                }
            };
            if let Some(text) = text {
                let remaining = self.options.probe_limit().saturating_sub(self.counters.matches.load(Ordering::Relaxed));
                collect_hits_limit(&mut local, path, rel, &text, &self.re, remaining, || self.stopped());
            }
        }
        if !local.is_empty() {
            batch.push(FileHits { path: path.to_path_buf(),
                rel: local[0].rel.clone(), matches: local.into_iter().flat_map(|f| f.matches).collect() });
        }
        if batch.len() >= 32 || batch.iter().map(|f| f.matches.len()).sum::<usize>() >= 256 {
            self.send_files(batch);
        }
    }
}

fn run_search(root: PathBuf, re: Regex, filter: FilterSet, overrides: Arc<HashMap<PathBuf, String>>,
    options: SearchOptions, counters: Arc<Counters>, tx: Sender<Update>) {
    let ctx = SearchContext { root: root.clone(), re, filter: filter.clone(),
        overrides: overrides.clone(), options, counters: counters.clone(), tx: tx.clone() };
    let mut keys: Vec<_> = overrides.keys().collect();
    keys.sort();
    for path in keys {
        if ctx.stopped() { break; }
        if !path.starts_with(&root) || crate::credentials::is_private_path(path) { continue; }
        let rel = relative(&root, path);
        if rel.split('/').any(|p| p == ".git") || !filter.matches_file(&rel) { continue; }
        let _ = tx.send(Update::Current(rel.clone()));
        counters.visited.fetch_add(1, Ordering::Relaxed);
        let mut hits = Results::default();
        if options.file_names && let Some(f) = ctx.name_hit(path, MatchKind::FileName) {
            hits.files.push(f); hits.matches += 1;
        }
        if options.content {
            counters.scanned.fetch_add(1, Ordering::Relaxed);
            let text = &overrides[path];
            if options.ignore_binary && memchr::memchr(0, text.as_bytes()).is_some() {
                counters.skipped_binary.fetch_add(1, Ordering::Relaxed);
            } else {
                collect_hits_limit(&mut hits.files, path, rel, text, &ctx.re,
                    options.probe_limit().saturating_sub(counters.matches.load(Ordering::Relaxed)), || ctx.stopped());
                hits.matches = hits.files.iter().map(|f| f.matches.len()).sum();
            }
        }
        counters.matches.fetch_add(hits.matches, Ordering::Relaxed);
        if !hits.files.is_empty() { let _ = tx.send(Update::Batch(hits.files)); }
    }
    if !ctx.stopped() {
        let walk_root = root.clone();
        let mut builder = WalkBuilder::new(&root);
        builder.hidden(false).git_ignore(true).git_global(true).git_exclude(true)
            .require_git(false).follow_links(false)
            .threads(std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8))
            .filter_entry(move |e| {
                if counters.cancel.load(Ordering::Relaxed) || e.file_name() == ".git"
                    || crate::credentials::is_private_path(e.path()) { return false; }
                if e.depth() > 0 && e.file_type().is_some_and(|t| t.is_dir()) {
                    return filter.allows_dir(&relative(&walk_root, e.path()));
                }
                true
            });
        let walker = builder.build_parallel();
        struct Builder(SearchContext);
        impl<'s> ignore::ParallelVisitorBuilder<'s> for Builder {
            fn build(&mut self) -> Box<dyn ignore::ParallelVisitor + 's> {
                let ctx = SearchContext { root: self.0.root.clone(), re: self.0.re.clone(),
                    filter: self.0.filter.clone(), overrides: self.0.overrides.clone(),
                    options: self.0.options, counters: self.0.counters.clone(), tx: self.0.tx.clone() };
                Box::new(Visitor { ctx, batch: Vec::new() })
            }
        }
        walker.visit(&mut Builder(ctx));
    }
    let _ = tx.send(Update::Done);
}

struct Visitor { ctx: SearchContext, batch: Vec<FileHits> }
impl ignore::ParallelVisitor for Visitor {
    fn visit(&mut self, entry: Result<ignore::DirEntry, ignore::Error>) -> ignore::WalkState {
        match entry {
            Ok(entry) => self.ctx.visit(&entry, &mut self.batch),
            Err(e) => self.ctx.warn(e.to_string()),
        }
        if self.ctx.stopped() {
            self.ctx.send_files(&mut self.batch);
            ignore::WalkState::Quit
        } else { ignore::WalkState::Continue }
    }
}
impl Drop for Visitor {
    fn drop(&mut self) { self.ctx.send_files(&mut self.batch); }
}

fn read_search_file(path: &Path, ignore_binary: bool, counters: &Counters) -> std::io::Result<Option<String>> {
    use std::io::Read;
    if crate::credentials::is_private_path(path) { return Ok(None); }
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_FILE_BYTES {
        counters.skipped_large.fetch_add(1, Ordering::Relaxed);
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        counters.skipped_large.fetch_add(1, Ordering::Relaxed);
        return Ok(None);
    }
    // SIMD NUL detection examines EVERY byte, not just an 8 KiB prefix.
    if ignore_binary && memchr::memchr(0, &bytes).is_some() {
        counters.skipped_binary.fetch_add(1, Ordering::Relaxed);
        return Ok(None);
    }
    let mut text = String::from_utf8(bytes).map_err(|e| std::io::Error::new(
        std::io::ErrorKind::InvalidData, format!("not UTF-8: {e}")))?;
    if text.contains("\r\n") { text = text.replace("\r\n", "\n"); }
    Ok(Some(text))
}

fn collect_hits_limit(out: &mut Vec<FileHits>, path: &Path, rel: String, text: &str,
    re: &Regex, limit: usize, cancelled: impl Fn() -> bool) {
    let mut hits = Vec::new();
    let mut total = 0;
    for (line_no, raw) in text.split('\n').enumerate() {
        if line_no % 64 == 0 && cancelled() { break; }
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        // Count a minified line once, then slide the preview window forward in
        // O(line + matches) instead of rescanning from the start for every hit.
        let line_chars = if line.len() > MAX_PREVIEW_CHARS { line.chars().count() } else { line.len() };
        let mut preview_byte = 0;
        let mut preview_char = 0;
        let mut scanned_byte = 0;
        let mut scanned_char = 0;
        for m in re.find_iter(line).filter(|m| !m.is_empty()) {
            if total >= limit { break; }
            scanned_char += line[scanned_byte..m.start()].chars().count();
            let start = scanned_char;
            let end = start + line[m.start()..m.end()].chars().count();
            scanned_char = end;
            scanned_byte = m.end();
            // Bounded previews for minified files; full columns remain valid.
            let (preview, offset) = if line_chars > MAX_PREVIEW_CHARS {
                let left = start.saturating_sub(MAX_PREVIEW_CHARS / 4);
                while preview_char < left {
                    let Some(ch) = line[preview_byte..].chars().next() else { break; };
                    preview_byte += ch.len_utf8();
                    preview_char += 1;
                }
                let mut preview: String = line[preview_byte..].chars().take(MAX_PREVIEW_CHARS).collect();
                let trim = preview.trim_end_matches('\r').len();
                preview.truncate(trim);
                (preview, preview_char)
            } else { (line.to_string(), 0) };
            hits.push(Hit { line: line_no, start, end, text: preview,
                text_offset: offset, kind: MatchKind::Content });
            total += 1;
            if total % 256 == 0 && cancelled() { break; }
        }
        if total >= limit { break; }
    }
    if !hits.is_empty() { out.push(FileHits { path: path.to_path_buf(), rel, matches: hits }); }
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
    fn binary_detection_checks_beyond_the_old_prefix() {
        let dir = std::env::temp_dir().join(format!("termed-nul-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("late.bin");
        let mut bytes = vec![b'x'; 16 * 1024];
        bytes.push(0); bytes.extend_from_slice(b"needle");
        std::fs::write(&path, &bytes).unwrap();
        assert!(read_search_file(&path, true, &Counters::default()).unwrap().is_none());
        assert!(read_search_file(&path, false, &Counters::default()).unwrap().is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn result_budget_has_no_hidden_per_file_cap() {
        let dir = std::env::temp_dir().join(format!("termed-budget-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("many.txt"), "needle\n".repeat(600)).unwrap();
        let options = SearchOptions { max_results: 500, ..Default::default() };
        let result = search_with_options(&dir, &Query::new("needle"), &FilterSet::default(), &HashMap::new(), &options).unwrap();
        assert_eq!(result.matches, 500);
        assert_eq!(result.files[0].matches.len(), 500);
        assert!(result.truncated);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn minified_unicode_previews_slide_without_rescanning() {
        let line = format!("{}needle{}needle", "é".repeat(3000), "é".repeat(3000));
        let re = Regex::new("needle").unwrap();
        let mut out = Results::default();
        collect_hits_limit(&mut out.files, Path::new("min.txt"), "min.txt".into(), &line, &re, 10, || false);
        let hits = &out.files[0].matches;
        assert_eq!(hits.len(), 2);
        assert_eq!((hits[0].start, hits[0].end), (3000, 3006));
        assert_eq!((hits[1].start, hits[1].end), (6006, 6012));
        for hit in hits {
            assert!(hit.text.chars().count() <= MAX_PREVIEW_CHARS);
            assert!(hit.text.contains("needle"));
            assert_eq!(hit.text_offset, hit.start.saturating_sub(MAX_PREVIEW_CHARS / 4));
        }
    }

    #[test]
    fn exact_result_limit_is_not_marked_truncated() {
        let dir = std::env::temp_dir().join(format!("termed-exact-limit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("five.txt"), "needle\n".repeat(5)).unwrap();
        let options = SearchOptions { max_results: 5, ..Default::default() };
        let result = search_with_options(&dir, &Query::new("needle"), &FilterSet::default(), &HashMap::new(), &options).unwrap();
        assert_eq!(result.matches, 5);
        assert!(!result.truncated);
        std::fs::remove_dir_all(dir).unwrap();
    }

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
        collect_hits_limit(&mut out.files, Path::new("unicode.txt"), "unicode.txt".into(), "aé éé", &re, 100, || false);
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
