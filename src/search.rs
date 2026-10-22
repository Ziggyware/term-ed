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
    /// Separators normalized to `/`.
    text: String,
    /// A trailing `/` or `\` in the filter: the pattern names a directory.
    dir: bool,
}

impl Glob {
    /// Match a project-relative path. Patterns apply to the file name and to
    /// the path, and to any trailing part of the path, so `src/*.rs` works
    /// anywhere in the tree and `*.cs` matches at every depth.
    fn matches(&self, rel: &str) -> bool {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        if glob_match(&self.text, name) || glob_match(&self.text, rel) {
            return true;
        }
        rel.match_indices('/')
            .any(|(i, _)| glob_match(&self.text, &rel[i + 1..]))
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
        if self.dir_includes.is_empty() {
            return true;
        }
        // Descend when the folder is inside a scope, or on the way to one
        // (`src` scopes the walk, so `src/app` must stay reachable).
        self.dir_includes.iter().any(|g| dir_reaches(rel, g))
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
        out.push((exclude, Glob { text, dir }));
    }
    out
}

/// Is the directory `rel` the include pattern itself, inside it, or on the
/// way to it? (`src` scopes the walk, so the walker may enter `src/app`.)
fn dir_reaches(rel: &str, g: &Glob) -> bool {
    if g.matches(rel) || in_dir(rel, g) {
        return true;
    }
    // A plain (wildcard-free) pattern can be reached by walking into its
    // leading folders. Globs are matched against folders as we go instead.
    if g.text.contains(['*', '?', '[']) {
        return false;
    }
    let prefix = format!("{rel}/");
    g.text.starts_with(&prefix) || rel.is_empty()
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

/// Match one glob against one path fragment.
///
/// `*` and `?` never cross a `/`, `**` does, and `[abc]` / `[a-z]` / `[!a-z]`
/// character classes are supported.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().map(fold).collect();
    let t: Vec<char> = text.chars().map(fold).collect();
    let mut budget = 200_000u32;
    gm(&p, &t, &mut budget)
}

fn gm(p: &[char], t: &[char], budget: &mut u32) -> bool {
    if *budget == 0 {
        // Pathological pattern: stop rather than hang the editor.
        return false;
    }
    *budget -= 1;
    let Some(&first) = p.first() else {
        return t.is_empty();
    };
    match first {
        '*' if p.get(1) == Some(&'*') => {
            let mut i = 2;
            while p.get(i) == Some(&'*') {
                i += 1;
            }
            let rest = &p[i..];
            // `**/` may also match no directory at all, so `**/x` finds `x`.
            if rest.first() == Some(&'/') && gm(&rest[1..], t, budget) {
                return true;
            }
            (0..=t.len()).any(|k| gm(rest, &t[k..], budget))
        }
        '*' => {
            let rest = &p[1..];
            let mut k = 0;
            loop {
                if gm(rest, &t[k..], budget) {
                    return true;
                }
                if k >= t.len() || t[k] == '/' {
                    return false;
                }
                k += 1;
            }
        }
        '?' => !t.is_empty() && t[0] != '/' && gm(&p[1..], &t[1..], budget),
        '[' => match class(p, t, budget) {
            Some(m) => m,
            // Unterminated class: treat the bracket literally.
            None => !t.is_empty() && t[0] == '[' && gm(&p[1..], &t[1..], budget),
        },
        c => !t.is_empty() && t[0] == c && gm(&p[1..], &t[1..], budget),
    }
}

/// Match a `[...]` class at the front of `p`; `None` if it never closes.
fn class(p: &[char], t: &[char], budget: &mut u32) -> Option<bool> {
    let mut i = 1;
    let negate = matches!(p.get(i), Some('!' | '^'));
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    let mut target = t.first().copied();
    if target == Some('/') {
        // A class never matches a path separator (as with `*` and `?`).
        target = None;
    }
    while i < p.len() {
        if p[i] == ']' && !first {
            let ok = target.is_some() && matched != negate;
            let skip = if target.is_some() { 1 } else { 0 };
            return Some(ok && gm(&p[i + 1..], &t[skip..], budget));
        }
        first = false;
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            if let Some(c) = target
                && c >= p[i]
                && c <= p[i + 2]
            {
                matched = true;
            }
            i += 3;
        } else {
            if target == Some(p[i]) {
                matched = true;
            }
            i += 1;
        }
    }
    None
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
        let meta = std::fs::metadata(path).ok()?;
        if meta.len() > MAX_FILE_BYTES {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        if looks_binary(&bytes) {
            return None;
        }
        let mut text = String::from_utf8(bytes).ok()?;
        let crlf = text.contains("\r\n");
        if crlf {
            text = text.replace("\r\n", "\n");
        }
        Some(Self {
            path: path.to_path_buf(),
            text,
            crlf,
        })
    }

    pub fn write(&self, text: &str) -> std::io::Result<()> {
        let out = if self.crlf && !text.contains("\r\n") {
            text.replace('\n', "\r\n")
        } else {
            text.to_string()
        };
        std::fs::write(&self.path, out)
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
        return project.to_path_buf();
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
    normalize(&joined)
}

/// Fold `.` and `..` without touching the filesystem (the path may not exist).
fn normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
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

    let walk_root = root.to_path_buf();
    let walk_filter = filter.clone();
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .filter_entry(move |e| {
            if e.file_name() == ".git" {
                return false;
            }
            if e.depth() > 0 && e.file_type().is_some_and(|t| t.is_dir()) {
                let rel = relative(&walk_root, e.path());
                return walk_filter.allows_dir(&rel);
            }
            true
        });

    for entry in builder.build().flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let rel = relative(root, path);
        if !filter.matches_file(&rel) {
            continue;
        }
        if entry.metadata().is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
            continue;
        }
        let text = match overrides.get(path) {
            Some(t) => t.clone(),
            None => match TextFile::read(path) {
                Some(f) => f.text,
                None => continue,
            },
        };
        out.scanned += 1;
        if out.scanned > MAX_FILES {
            out.truncated = true;
            break;
        }
        let mut hits = Vec::new();
        for (line_no, raw) in text.split('\n').enumerate() {
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            if hits.len() >= MAX_MATCHES_PER_FILE {
                out.truncated = true;
                break;
            }
            for m in re.find_iter(line) {
                if m.start() == m.end() {
                    continue; // empty match: nothing to show or replace
                }
                let start = line[..m.start()].chars().count();
                let end = start + line[m.start()..m.end()].chars().count();
                hits.push(Hit {
                    line: line_no,
                    start,
                    end,
                    text: line.to_string(),
                });
                out.matches += 1;
                if out.matches >= MAX_MATCHES || hits.len() >= MAX_MATCHES_PER_FILE {
                    out.truncated = true;
                    break;
                }
            }
            if out.truncated && (out.matches >= MAX_MATCHES || hits.len() >= MAX_MATCHES_PER_FILE) {
                break;
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
            break;
        }
    }
    out.files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
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
        if n > 0 {
            out.push_str(&new);
        } else {
            out.push_str(line);
        }
    }
    (out, count)
}

fn replace_in_line(line: &str, re: &Regex, replacement: &str, literal: bool) -> (String, usize) {
    let mut out = String::with_capacity(line.len());
    let mut last = 0;
    let mut count = 0;
    for caps in re.captures_iter(line) {
        let Some(m) = caps.get(0) else { continue };
        // Empty matches are never listed in the results, so they are not
        // replaced either (otherwise `x*` would insert text everywhere).
        if m.start() == m.end() {
            continue;
        }
        out.push_str(&line[last..m.start()]);
        if literal {
            out.push_str(replacement);
        } else {
            let mut dst = String::new();
            caps.expand(replacement, &mut dst);
            out.push_str(&dst);
        }
        last = m.end();
        count += 1;
    }
    if count == 0 {
        return (line.to_string(), 0);
    }
    out.push_str(&line[last..]);
    (out, count)
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
    for m in re.find_iter(line) {
        if m.start() == m.end() {
            continue;
        }
        if line[..m.start()].chars().count() != start_char {
            continue;
        }
        let rep = if literal {
            replacement.to_string()
        } else {
            let caps = re.captures_at(line, m.start())?;
            let mut dst = String::new();
            caps.expand(replacement, &mut dst);
            dst
        };
        return Some(format!("{}{}{}", &line[..m.start()], rep, &line[m.end()..]));
    }
    None
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
        assert!(!f.allows_dir("tests"));
        // On the way to a scope, the walker may descend.
        let f = FilterSet::parse_parts("src/app", "*");
        assert!(f.allows_dir("src"));
        assert!(!f.allows_dir("tests"));

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
