//! ZIPs are virtual folders (`bundle.zip/src/main.rs`), never auto-extracted.
//! Reads are bounded; edits rebuild beside the original and commit atomically.
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_MEMBER_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    len: u64,
    modified: SystemTime,
}
impl Stamp {
    pub fn read(path: &Path) -> Result<Self> {
        let m = fs::metadata(path)?;
        if m.len() > MAX_ARCHIVE_BYTES {
            bail!("ZIP exceeds the 256 MB archive limit");
        }
        Ok(Self {
            len: m.len(),
            modified: m.modified()?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct MemberPath {
    pub archive: PathBuf,
    pub member: String,
}

pub fn is_zip(path: &Path) -> bool {
    path.extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("zip"))
}

/// A real ZIP file must be an ancestor; an ordinary directory named `.zip`
/// stays an ordinary directory. Nested ZIP containers are not mounted.
pub fn member_path(path: &Path) -> Option<MemberPath> {
    for parent in path.ancestors().skip(1) {
        if is_zip(parent) && parent.is_file() {
            let member = path.strip_prefix(parent).ok()?.to_str()?.replace('\\', "/");
            return Some(MemberPath {
                archive: parent
                    .canonicalize()
                    .unwrap_or_else(|_| parent.to_path_buf()),
                member,
            });
        }
    }
    None
}

/// Refuse traversal, absolute/drive paths, control characters and ambiguous
/// separator aliases on every platform, not only the machine opening the ZIP.
fn safe_name(name: &str) -> Result<&str> {
    if name.is_empty()
        || name.len() > 2048
        || name.starts_with('/')
        || name.ends_with("//")
        || name.contains(['\\', ':'])
        || name.chars().any(char::is_control)
        || name
            .trim_end_matches('/')
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        bail!("ZIP contains an unsafe or ambiguous member path");
    }
    Ok(name.trim_end_matches('/'))
}

/// Inspect the bounded footer before the ZIP library allocates its index.
/// ZIP64 counts are checked too, even when ordinary EOCD fields are not sentinels.
fn open_zip(path: &Path) -> Result<ZipArchive<File>> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len > MAX_ARCHIVE_BYTES {
        bail!("ZIP exceeds the 256 MB archive limit");
    }
    let tail_len = len.min(22 + u64::from(u16::MAX));
    file.seek(SeekFrom::End(-(tail_len as i64)))?;
    let mut tail = Vec::new();
    (&mut file).take(tail_len).read_to_end(&mut tail)?;
    let end = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&at| {
            tail[at..].starts_with(b"PK\x05\x06")
                && at + 22 + usize::from(u16::from_le_bytes([tail[at + 20], tail[at + 21]]))
                    == tail.len()
        })
        .context("Missing or invalid ZIP end-of-directory record")?;
    for at in [end + 8, end + 10] {
        let count = u16::from_le_bytes([tail[at], tail[at + 1]]);
        if count != u16::MAX && usize::from(count) > MAX_ENTRIES {
            bail!("ZIP has more than 10,000 entries");
        }
    }
    let end_offset = len - tail_len + end as u64;
    let mut zip64 = false;
    if let Some(locator_at) = end_offset.checked_sub(20) {
        let mut locator = [0u8; 20];
        file.seek(SeekFrom::Start(locator_at))?;
        file.read_exact(&mut locator)?;
        if locator.starts_with(b"PK\x06\x07") {
            zip64 = true;
            let offset = u64::from_le_bytes(locator[8..16].try_into()?);
            let mut record = [0u8; 56];
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(&mut record)
                .context("Invalid ZIP64 directory")?;
            let size = u64::from_le_bytes(record[4..12].try_into()?);
            if !record.starts_with(b"PK\x06\x06")
                || size < 44
                || offset.checked_add(12).and_then(|n| n.checked_add(size)) != Some(locator_at)
            {
                bail!("Invalid or unsupported ZIP64 directory offset");
            }
            for at in [24, 32] {
                if u64::from_le_bytes(record[at..at + 8].try_into()?) > MAX_ENTRIES as u64 {
                    bail!("ZIP has more than 10,000 entries");
                }
            }
        }
    }
    if !zip64
        && (tail[end + 8..end + 12]
            .chunks_exact(2)
            .any(|v| v == [255, 255]))
    {
        bail!("Missing ZIP64 directory");
    }
    file.seek(SeekFrom::Start(0))?;
    ZipArchive::new(file).context("Invalid or unsupported ZIP")
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub directory: bool,
    pub size: u64,
    pub compressed_size: u64,
    pub symlink: bool,
    pub crc32: u32,
}
#[derive(Clone, Debug)]
pub struct Index {
    pub entries: Vec<Entry>,
    pub stamp: Stamp,
}
impl Index {
    pub fn open(path: &Path) -> Result<Self> {
        let stamp = Stamp::read(path)?;
        let mut zip = open_zip(path)?;
        if zip.len() > MAX_ENTRIES {
            bail!("ZIP has more than 10,000 entries");
        }
        let mut names = HashSet::new();
        let mut entries = Vec::with_capacity(zip.len());
        for i in 0..zip.len() {
            let file = zip.by_index_raw(i)?;
            let name = safe_name(file.name())?.to_string();
            if !names.insert(name.clone()) {
                bail!("ZIP contains duplicate member paths");
            }
            entries.push(Entry {
                name,
                directory: file.is_dir(),
                size: file.size(),
                compressed_size: file.compressed_size(),
                crc32: file.crc32(),
                symlink: file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000),
            });
        }
        if Stamp::read(path)? != stamp {
            bail!("ZIP changed while its directory was being read; refresh it");
        }
        // A file may not also be an implicit directory.
        let files: HashSet<_> = entries
            .iter()
            .filter(|e| !e.directory)
            .map(|e| e.name.as_str())
            .collect();
        for entry in &entries {
            for (at, _) in entry.name.match_indices('/') {
                if files.contains(&entry.name[..at]) {
                    bail!("ZIP contains conflicting file and folder paths");
                }
            }
        }
        Ok(Self { entries, stamp })
    }

    /// Immediate children, including folders omitted from the central directory.
    pub fn children(&self, prefix: &str) -> Vec<(String, bool)> {
        let prefix = if prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", prefix.trim_end_matches('/'))
        };
        let mut children = BTreeMap::<String, bool>::new();
        for entry in &self.entries {
            let Some(rest) = entry.name.strip_prefix(&prefix) else {
                continue;
            };
            if rest.is_empty() {
                continue;
            }
            let (name, directory) = rest
                .split_once('/')
                .map_or((rest, entry.directory), |(n, _)| (n, true));
            children
                .entry(name.into())
                .and_modify(|d| *d |= directory)
                .or_insert(directory);
        }
        let mut out: Vec<_> = children.into_iter().collect();
        out.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
                .then(a.0.cmp(&b.0))
        });
        out
    }

    pub fn summary(&self, path: &Path) -> String {
        let files = self.entries.iter().filter(|e| !e.directory).count();
        let size: u64 = self
            .entries
            .iter()
            .fold(0u64, |n, e| n.saturating_add(e.size));
        let mut s = format!(
            "ZIP ARCHIVE\n{}\n\n{} files · {} entries · {} expanded\n\nBrowse: expand the ZIP in Explorer with Right or Enter.\nOpen a text member to edit; Ctrl+S updates this ZIP atomically.\nImage members open in the image viewer. Save As exports a member.\nSave As on this overview copies the ZIP file, not this listing.\n\nMEMBERS\n",
            path.file_name().unwrap_or_default().to_string_lossy(),
            files,
            self.entries.len(),
            byte_label(size)
        );
        for e in self.entries.iter().take(200) {
            s.push_str(&format!(
                "{}  {}{}{}\n",
                byte_label(e.size),
                e.name,
                if e.directory { "/" } else { "" },
                if e.symlink {
                    " [link; not followed]"
                } else {
                    ""
                }
            ));
        }
        if self.entries.len() > 200 {
            s.push_str("… listing truncated; use Explorer for the full directory.\n");
        }
        s
    }
}

#[derive(Clone, Debug)]
pub struct MemberVersion {
    pub stamp: Stamp,
    pub crc32: Option<u32>,
}

pub fn member_version(path: &MemberPath) -> Result<MemberVersion> {
    let index = Index::open(&path.archive)?;
    let crc32 = index
        .entries
        .iter()
        .find(|e| e.name == path.member)
        .map(|e| e.crc32);
    Ok(MemberVersion {
        stamp: index.stamp,
        crc32,
    })
}

pub fn read_member(path: &MemberPath) -> Result<(Vec<u8>, MemberVersion)> {
    let name = safe_name(&path.member)?;
    let index = Index::open(&path.archive)?;
    let entry = index
        .entries
        .iter()
        .find(|e| e.name == name)
        .context("ZIP member does not exist")?;
    if entry.directory {
        bail!("This ZIP member is a folder; expand it in Explorer");
    }
    if entry.symlink {
        bail!("ZIP symlinks are not followed");
    }
    if entry.size > MAX_MEMBER_BYTES {
        bail!("ZIP member exceeds the 32 MB expanded limit");
    }
    let mut zip = open_zip(&path.archive)?;
    let mut data = Vec::with_capacity(entry.size as usize);
    zip.by_name(name)?
        .take(MAX_MEMBER_BYTES + 1)
        .read_to_end(&mut data)
        .context("Cannot decode ZIP member (encrypted, unsupported compression or corrupt CRC)")?;
    if data.len() as u64 > MAX_MEMBER_BYTES {
        bail!("ZIP member exceeds the 32 MB expanded limit");
    }
    if Stamp::read(&path.archive)? != index.stamp {
        bail!("ZIP changed while reading; reopen the member");
    }
    Ok((
        data,
        MemberVersion {
            stamp: index.stamp,
            crc32: Some(entry.crc32),
        },
    ))
}

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn write_member(
    path: &MemberPath,
    expected: &MemberVersion,
    data: &[u8],
) -> Result<MemberVersion> {
    let name = safe_name(&path.member)?;
    if data.len() as u64 > MAX_MEMBER_BYTES {
        bail!("ZIP member exceeds the 32 MB expanded limit");
    }
    let index = Index::open(&path.archive)?;
    let entry = index.entries.iter().find(|e| e.name == name);
    // Rebase safely when another tab/process changed a different member.
    if entry.map(|e| e.crc32) != expected.crc32 {
        bail!(
            "ZIP member changed on disk; your edits were kept. Reopen it or Save As to export them"
        );
    }
    if entry.is_some_and(|e| e.directory || e.symlink) {
        bail!("Cannot edit ZIP folders or links");
    }
    if entry.is_none() && index.entries.len() >= MAX_ENTRIES {
        bail!("ZIP has reached the 10,000 entry limit");
    }
    for e in &index.entries {
        if e.name.starts_with(&format!("{name}/"))
            || (!e.directory && name.starts_with(&format!("{}/", e.name)))
        {
            bail!("ZIP file/folder path conflict");
        }
    }
    // zip's raw-copy API does not retain link types or encryption metadata.
    // Refuse rather than silently damage an unrelated entry while saving text.
    if index.entries.iter().any(|e| e.symlink) {
        bail!("ZIPs containing symlinks are browse-only; Save As to export your text edits");
    }
    let permissions = fs::metadata(&path.archive)?.permissions();
    if permissions.readonly() {
        bail!("ZIP file is read-only");
    }
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp_path = path.archive.with_file_name(format!(
        ".{}.termed-{}-{}.tmp",
        path.archive
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)?;
    let tmp = Temp(tmp_path);
    {
        let mut input = open_zip(&path.archive)?;
        let mut writer = ZipWriter::new(output);
        writer.set_raw_comment(input.comment().to_vec().into());
        for i in 0..input.len() {
            let file = input.by_index_raw(i)?;
            if file.encrypted() {
                bail!(
                    "ZIPs containing encrypted members are browse-only; Save As to export your text edits"
                );
            }
            if file.name() == name {
                let options = SimpleFileOptions::default()
                    .compression_method(CompressionMethod::Deflated)
                    .last_modified_time(file.last_modified().unwrap_or_default())
                    .unix_permissions(file.unix_mode().unwrap_or(0o644));
                writer.start_file(name, options)?;
                writer.write_all(data)?;
            } else {
                writer.raw_copy_file(file)?;
            }
        }
        if entry.is_none() {
            writer.start_file(
                name,
                SimpleFileOptions::default()
                    .compression_method(CompressionMethod::Deflated)
                    .unix_permissions(0o644),
            )?;
            writer.write_all(data)?;
        }
        let output = writer.finish()?;
        output.sync_all()?;
        fs::set_permissions(&tmp.0, permissions)?;
    }
    if fs::metadata(&tmp.0)?.len() > MAX_ARCHIVE_BYTES {
        bail!("Updated ZIP would exceed the 256 MB limit; original was kept");
    }
    if Stamp::read(&path.archive)? != index.stamp {
        bail!("ZIP changed while saving; original and unsaved edits were kept");
    }
    fs::rename(&tmp.0, &path.archive)
        .context("Cannot replace ZIP; original and unsaved edits were kept")?;
    member_version(path)
}

pub fn byte_label(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1048576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}
