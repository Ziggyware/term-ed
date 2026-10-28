//! Text document: a rope buffer plus undo/redo history and file metadata.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ropey::Rope;

/// A selection expressed in char offsets. `head` is where the caret is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn caret(at: usize) -> Self {
        Self {
            anchor: at,
            head: at,
        }
    }
    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }
    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }
    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

#[derive(Clone, Debug)]
enum Change {
    Insert { at: usize, text: String },
    Delete { at: usize, text: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    Typing,
    Deleting,
    Other,
}

#[derive(Clone, Debug)]
struct Transaction {
    changes: Vec<Change>,
    before: Selection,
    after: Selection,
    kind: EditKind,
    at: Instant,
    rev: u64,
    prev_rev: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn label(self) -> &'static str {
        match self {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indent {
    Spaces(usize),
    Tabs,
}

impl Indent {
    pub fn unit(self) -> String {
        match self {
            Indent::Spaces(n) => " ".repeat(n),
            Indent::Tabs => "\t".to_string(),
        }
    }
    pub fn label(self) -> String {
        match self {
            Indent::Spaces(n) => format!("Spaces: {n}"),
            Indent::Tabs => "Tabs".to_string(),
        }
    }
}

pub enum Preview {
    Image(crate::image_view::ImageView),
    Archive(crate::archive::Index),
}

pub struct Document {
    pub preview: Option<Preview>,
    archive_stamp: Option<crate::archive::MemberVersion>,
    /// Stable for this buffer's lifetime, even when tabs move or paths change.
    pub(crate) id: u64,
    pub rope: Rope,
    pub path: Option<PathBuf>,
    pub line_ending: LineEnding,
    pub indent: Indent,
    pub tab_width: usize,
    /// Text as it was when last loaded/saved; used for gutter change markers.
    pub saved_text: String,
    /// Bumped on every mutation; consumers use it to invalidate caches.
    pub version: u64,
    /// Lowest line touched since the consumer last called `take_dirty_line`.
    dirty_from_line: Option<usize>,
    rev: u64,
    next_rev: u64,
    saved_rev: u64,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    pending: Option<Transaction>,
}

impl Document {
    pub fn empty() -> Self {
        Self::from_text("", None)
    }

    pub fn open(path: &Path) -> Result<Self> {
        if path.is_file() && crate::archive::is_zip(path) {
            let index = crate::archive::Index::open(path)?;
            let mut doc = Self::from_text(&index.summary(path), Some(path.to_path_buf()));
            doc.preview = Some(Preview::Archive(index));
            return Ok(doc);
        }
        let (bytes, stamp) = if let Some(member) = crate::archive::member_path(path) {
            let (bytes, stamp) = crate::archive::read_member(&member)?;
            (bytes, Some(stamp))
        } else if path.exists() {
            // Never allocate an unbounded binary blob simply to discover it
            // is an image. Text documents keep the same input budget.
            use std::io::Read;
            let mut bytes = Vec::new();
            fs::File::open(path)?
                .take(crate::image_view::MAX_IMAGE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > crate::image_view::MAX_IMAGE_BYTES {
                anyhow::bail!("File exceeds the 32 MB open limit");
            }
            (bytes, None)
        } else {
            (Vec::new(), None)
        };
        let mut doc = if (crate::image_view::image_extension(path)
            && (!bytes.is_empty() || path.exists() || stamp.is_some()))
            || image::guess_format(&bytes).is_ok()
        {
            let preview = crate::image_view::ImageView::decode(bytes)?;
            let mut doc = Self::from_text("", Some(path.to_path_buf()));
            doc.preview = Some(Preview::Image(preview));
            doc
        } else {
            if bytes.contains(&0) {
                anyhow::bail!(
                    "Binary file is not a supported image or ZIP; it was not opened as text"
                );
            }
            let text = std::str::from_utf8(&bytes)
                .context("File is not UTF-8 text or a supported image")?;
            Self::from_text(text, Some(path.to_path_buf()))
        };
        doc.archive_stamp = stamp;
        Ok(doc)
    }

    pub fn is_read_only(&self) -> bool {
        self.preview.is_some()
    }

    /// Save As for a binary preview exports the original bytes, never a text
    /// listing or a decoded/re-encoded approximation of the image.
    pub fn export_preview(&self, path: &Path) -> Result<()> {
        if crate::archive::member_path(path).is_some() {
            anyhow::bail!("Export to a regular filesystem path, not inside a ZIP");
        }
        if self.path.as_ref().is_some_and(|p| same_file(p, path))
            || self
                .path
                .as_ref()
                .and_then(|p| crate::archive::member_path(p))
                .is_some_and(|p| same_file(&p.archive, path))
        {
            anyhow::bail!("Choose a different path for the exported file");
        }
        match &self.preview {
            Some(Preview::Image(image)) => {
                fs::write(path, &image.original)?;
            }
            Some(Preview::Archive(index)) => {
                let source = self.path.as_ref().context("ZIP has no path")?;
                if crate::archive::Stamp::read(source)? != index.stamp {
                    anyhow::bail!("ZIP changed on disk; reopen before exporting");
                }
                fs::copy(source, path)?;
            }
            None => anyhow::bail!("Not a binary preview"),
        }
        Ok(())
    }

    pub fn save_as(&mut self, path: &Path) -> Result<()> {
        if self.is_read_only() {
            return self.export_preview(path);
        }
        let stamp = if self.path.as_deref() == Some(path) && self.archive_stamp.is_some() {
            self.archive_stamp.clone()
        } else {
            crate::archive::member_path(path)
                .map(|m| crate::archive::member_version(&m))
                .transpose()?
        };
        let old_path = self.path.replace(path.to_path_buf());
        let old_stamp = std::mem::replace(&mut self.archive_stamp, stamp);
        if let Err(e) = self.save() {
            self.path = old_path;
            self.archive_stamp = old_stamp;
            return Err(e);
        }
        Ok(())
    }

    pub fn from_text(text: &str, path: Option<PathBuf>) -> Self {
        let line_ending = if text.contains("\r\n") {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        };
        let normalized = text.replace("\r\n", "\n");
        let indent = detect_indent(&normalized);
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            preview: None,
            archive_stamp: None,
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            rope: Rope::from_str(&normalized),
            path,
            line_ending,
            indent,
            tab_width: 4,
            saved_text: normalized,
            version: 0,
            dirty_from_line: None,
            rev: 0,
            next_rev: 1,
            saved_rev: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            pending: None,
        }
    }

    pub fn name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_string())
    }

    pub fn is_dirty(&self) -> bool {
        self.rev != self.saved_rev || self.pending.as_ref().is_some_and(|t| !t.changes.is_empty())
    }

    pub fn save(&mut self) -> Result<()> {
        if self.is_read_only() {
            anyhow::bail!("Preview is read-only; use Save As to export the original file");
        }
        self.commit();
        let path = self.path.clone().context("document has no path")?;
        let baseline = self.rope.to_string();
        let text = if self.line_ending == LineEnding::CrLf {
            baseline.replace('\n', "\r\n")
        } else {
            baseline.clone()
        };
        let member = crate::archive::member_path(&path);
        if self.archive_stamp.is_some() && member.is_none() {
            anyhow::bail!("ZIP container is missing or no longer a file; your edits were kept");
        }
        if let Some(member) = member {
            let stamp = self
                .archive_stamp
                .clone()
                .context("Reopen the ZIP member before saving")?;
            self.archive_stamp = Some(crate::archive::write_member(
                &member,
                &stamp,
                text.as_bytes(),
            )?);
        } else {
            ensure_text_target(&path)?;
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
                && !parent.exists()
            {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        }
        self.saved_text = baseline;
        self.saved_rev = self.rev;
        self.version += 1;
        Ok(())
    }

    /// Record that the buffer now matches the file on disk, without moving
    /// the change-bar baseline (so edits made by an agent stay visible).
    pub fn mark_saved(&mut self) {
        self.commit();
        self.saved_rev = self.rev;
    }

    /// Flag the document as needing a save without changing its text
    /// (e.g. after switching line endings).
    pub fn mark_modified(&mut self) {
        if self.is_read_only() {
            return;
        }
        self.saved_rev = u64::MAX;
        self.version += 1;
    }

    /// Returns (and clears) the first line that changed since the last call.
    pub fn take_dirty_line(&mut self) -> Option<usize> {
        self.dirty_from_line.take()
    }

    fn mark_dirty(&mut self, char_idx: usize) {
        let line = self.rope.char_to_line(char_idx.min(self.rope.len_chars()));
        self.dirty_from_line = Some(self.dirty_from_line.map_or(line, |l| l.min(line)));
        self.version += 1;
    }

    // ---------------------------------------------------------------- editing

    /// Start a transaction. Consecutive transactions of the same kind are merged
    /// (so typing a word undoes as one unit, like Visual Studio).
    pub fn begin(&mut self, kind: EditKind, sel: Selection) {
        if self.is_read_only() {
            return;
        }
        if let Some(p) = &self.pending {
            let mergeable = kind != EditKind::Other
                && p.kind == kind
                && p.after == sel
                && p.at.elapsed() < Duration::from_millis(1200);
            if mergeable {
                return;
            }
            self.commit();
        }
        self.pending = Some(Transaction {
            changes: Vec::new(),
            before: sel,
            after: sel,
            kind,
            at: Instant::now(),
            rev: 0,
            prev_rev: self.rev,
        });
    }

    /// Finish the current transaction, recording the resulting selection.
    pub fn end(&mut self, sel: Selection) {
        if let Some(p) = &mut self.pending {
            p.after = sel;
            p.at = Instant::now();
            if p.kind == EditKind::Other {
                self.commit();
            }
        }
    }

    /// Seal the pending transaction so the next edit starts a new undo step.
    pub fn commit(&mut self) {
        if let Some(mut t) = self.pending.take() {
            if t.changes.is_empty() {
                return;
            }
            t.rev = self.next_rev;
            self.next_rev += 1;
            self.rev = t.rev;
            self.undo.push(t);
            self.redo.clear();
        }
    }

    pub fn insert(&mut self, at: usize, text: &str) {
        if self.is_read_only() {
            return;
        }
        if text.is_empty() {
            return;
        }
        let at = at.min(self.rope.len_chars());
        self.rope.insert(at, text);
        self.mark_dirty(at);
        self.record(Change::Insert {
            at,
            text: text.to_string(),
        });
    }

    pub fn remove(&mut self, start: usize, end: usize) {
        if self.is_read_only() {
            return;
        }
        let end = end.min(self.rope.len_chars());
        if start >= end {
            return;
        }
        let text = self.rope.slice(start..end).to_string();
        self.rope.remove(start..end);
        self.mark_dirty(start);
        self.record(Change::Delete { at: start, text });
    }

    fn record(&mut self, change: Change) {
        if self.pending.is_none() {
            self.begin(EditKind::Other, Selection::default());
        }
        if let Some(p) = &mut self.pending {
            p.changes.push(change);
        }
    }

    pub fn undo(&mut self) -> Option<Selection> {
        if self.is_read_only() {
            return None;
        }
        self.commit();
        let t = self.undo.pop()?;
        for c in t.changes.iter().rev() {
            match c {
                Change::Insert { at, text } => {
                    let end = at + text.chars().count();
                    self.rope.remove(*at..end);
                    self.mark_dirty(*at);
                }
                Change::Delete { at, text } => {
                    self.rope.insert(*at, text);
                    self.mark_dirty(*at);
                }
            }
        }
        self.rev = t.prev_rev;
        let sel = t.before;
        self.redo.push(t);
        Some(sel)
    }

    pub fn redo(&mut self) -> Option<Selection> {
        if self.is_read_only() {
            return None;
        }
        self.commit();
        let t = self.redo.pop()?;
        for c in &t.changes {
            match c {
                Change::Insert { at, text } => {
                    self.rope.insert(*at, text);
                    self.mark_dirty(*at);
                }
                Change::Delete { at, text } => {
                    let end = at + text.chars().count();
                    self.rope.remove(*at..end);
                    self.mark_dirty(*at);
                }
            }
        }
        self.rev = t.rev;
        let sel = t.after;
        self.undo.push(t);
        Some(sel)
    }

    // ---------------------------------------------------------------- queries

    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    /// Line content without the trailing newline.
    pub fn line_str(&self, line: usize) -> String {
        if line >= self.rope.len_lines() {
            return String::new();
        }
        let mut s = self.rope.line(line).to_string();
        while s.ends_with('\n') || s.ends_with('\r') {
            s.pop();
        }
        s
    }

    /// Number of chars in a line, excluding the newline.
    pub fn line_len(&self, line: usize) -> usize {
        if line >= self.rope.len_lines() {
            return 0;
        }
        let slice = self.rope.line(line);
        let mut n = slice.len_chars();
        if n > 0 && slice.char(n - 1) == '\n' {
            n -= 1;
        }
        n
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.rope.line_to_char(line.min(self.rope.len_lines()))
    }

    /// (line, column-in-chars) for a char offset.
    pub fn pos(&self, idx: usize) -> (usize, usize) {
        let idx = idx.min(self.rope.len_chars());
        let line = self.rope.char_to_line(idx);
        (line, idx - self.rope.line_to_char(line))
    }

    pub fn idx(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.rope.len_lines().saturating_sub(1));
        self.line_start(line) + col.min(self.line_len(line))
    }

    pub fn char_at(&self, idx: usize) -> Option<char> {
        (idx < self.rope.len_chars()).then(|| self.rope.char(idx))
    }
}

fn detect_indent(text: &str) -> Indent {
    let mut tabs = 0usize;
    let mut spaces = [0usize; 9];
    let mut prev = 0usize;
    for line in text.lines().take(2000) {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with('\t') {
            tabs += 1;
            continue;
        }
        let n = line.len() - line.trim_start_matches(' ').len();
        let delta = n.abs_diff(prev);
        if (2..=8).contains(&delta) {
            spaces[delta] += 1;
        }
        prev = n;
    }
    let (best, count) = spaces
        .iter()
        .enumerate()
        .max_by_key(|(_, c)| **c)
        .map(|(i, c)| (i, *c))
        .unwrap();
    if tabs > count {
        Indent::Tabs
    } else if count > 0 {
        Indent::Spaces(best)
    } else {
        Indent::Spaces(4)
    }
}

/// Text-only writes must not silently replace an image or its ZIP container,
/// including aliases whose extension no longer identifies the binary format.
pub(crate) fn ensure_text_target(path: &Path) -> Result<()> {
    if path
        .ancestors()
        .skip(1)
        .any(|p| crate::archive::is_zip(p) && !p.exists())
    {
        anyhow::bail!("ZIP container is missing; choose an existing ZIP or a regular directory");
    }
    if !path.is_file() {
        return Ok(());
    }
    use std::io::Read;
    let mut prefix = Vec::new();
    fs::File::open(path)?.take(32).read_to_end(&mut prefix)?;
    if crate::archive::is_zip(path)
        || image::guess_format(&prefix).is_ok()
        || [b"PK\x03\x04", b"PK\x05\x06", b"PK\x07\x08"]
            .iter()
            .any(|m| prefix.starts_with(*m))
    {
        anyhow::bail!(
            "Cannot overwrite an image or ZIP with text; choose a text file or a ZIP member path"
        );
    }
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    if crate::search::absolute_path(a) == crate::search::absolute_path(b) {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(a), Ok(b)) = (fs::metadata(a), fs::metadata(b)) {
            return a.dev() == b.dev() && a.ino() == b.ino();
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_roundtrip() {
        let mut d = Document::from_text("hello\nworld", None);
        d.begin(EditKind::Other, Selection::caret(0));
        d.insert(5, ", there");
        d.end(Selection::caret(12));
        assert_eq!(d.rope.to_string(), "hello, there\nworld");
        assert!(d.is_dirty());
        assert_eq!(d.undo(), Some(Selection::caret(0)));
        assert_eq!(d.rope.to_string(), "hello\nworld");
        assert!(!d.is_dirty());
        d.redo();
        assert_eq!(d.rope.to_string(), "hello, there\nworld");
    }

    #[test]
    fn typing_coalesces() {
        let mut d = Document::empty();
        for (i, c) in "abc".chars().enumerate() {
            d.begin(EditKind::Typing, Selection::caret(i));
            d.insert(i, &c.to_string());
            d.end(Selection::caret(i + 1));
        }
        d.undo();
        assert_eq!(d.rope.to_string(), "");
    }

    #[test]
    fn crlf_is_normalized_and_detected() {
        let d = Document::from_text("a\r\nb\r\n", None);
        assert_eq!(d.line_ending, LineEnding::CrLf);
        assert_eq!(d.line_str(0), "a");
        assert_eq!(d.line_len(1), 1);
    }

    #[test]
    fn detects_indent() {
        assert_eq!(detect_indent("a\n  b\n    c\n  d\n"), Indent::Spaces(2));
        assert_eq!(detect_indent("a\n\tb\n\t\tc\n"), Indent::Tabs);
    }
}
