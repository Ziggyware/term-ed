//! User-local credentials. Never serialized into workspace preferences or prompts.
//! This is an owner-only plaintext file, not an OS keychain or an execution sandbox.
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde_json::json;

pub const ROUTER_ENV: &str = "ROUTER_API_KEY";
pub const MAX_KEY_BYTES: usize = 4096;

pub fn path() -> PathBuf {
    crate::config::config_dir().join("credentials.json")
}

/// Avoid accidentally sending credentials through the agent's file/context APIs,
/// including when the user opens their home directory as a workspace.
pub fn is_private_path(candidate: &Path) -> bool {
    let secret = path();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(a), Ok(b)) = (fs::metadata(candidate), fs::metadata(&secret)) {
            if a.dev() == b.dev() && a.ino() == b.ino() {
                return true;
            }
        }
    }
    crate::search::absolute_path(candidate) == crate::search::absolute_path(&secret)
        || candidate
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(".credentials-"))
}

pub fn validate_key(key: &str) -> Result<&str> {
    let key = key.trim();
    if key.is_empty() || key.len() > MAX_KEY_BYTES || key.chars().any(char::is_control) {
        bail!("Enter a nonempty router API key of at most 4096 bytes, without control characters");
    }
    Ok(key)
}

/// Never derives Debug/Serialize: a resolved key must not appear in settings or logs.
pub struct Secret(String);
impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

pub struct Store {
    file: PathBuf,
}
impl Default for Store {
    fn default() -> Self {
        Self::new(path())
    }
}
impl Store {
    pub fn new(file: PathBuf) -> Self {
        Self { file }
    }
    fn check_file(&self) -> Result<bool> {
        let metadata = match fs::symlink_metadata(&self.file) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e).context("Cannot inspect local credential file"),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16 * 1024 {
            bail!("Local credential file must be a small regular file, not a symlink");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if metadata.permissions().mode() & 0o077 != 0 || metadata.nlink() != 1 {
                bail!(
                    "Local credential file must have owner-only permissions (chmod 600) and no hard links"
                );
            }
        }
        Ok(true)
    }
    pub fn load(&self) -> Result<Option<Secret>> {
        if !self.check_file()? {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        fs::File::open(&self.file)?
            .take(16 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 16 * 1024 {
            bail!("Local credential file exceeds its size limit");
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
            anyhow::anyhow!("Invalid local credential JSON; remove or replace the saved key")
        })?;
        if value.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
            bail!("Unsupported local credential file version");
        }
        let key = value
            .get("router_api_key")
            .and_then(serde_json::Value::as_str)
            .context("Local credential file is missing router_api_key")?;
        Ok(Some(Secret(validate_key(key)?.to_string())))
    }
    pub fn save(&self, key: &str) -> Result<()> {
        let key = validate_key(key)?;
        self.check_file()?;
        let parent = self
            .file
            .parent()
            .context("Credential file has no parent")?;
        if fs::symlink_metadata(parent).is_ok_and(|m| m.file_type().is_symlink()) {
            bail!("Credential directory must not be a symlink");
        }
        if parent.ancestors().any(|p| {
            p.canonicalize()
                .unwrap_or_else(|_| p.to_path_buf())
                .ancestors()
                .any(|real| real.join(".git").exists())
        }) {
            bail!(
                "User credential directory is inside a Git checkout. Set XDG_CONFIG_HOME to a private directory outside the checkout"
            );
        }
        fs::create_dir_all(parent).context("Cannot create user credential directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let tmp = parent.join(format!(
            ".credentials-{}-{}.tmp",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| -> Result<()> {
            let mut file = options
                .open(&tmp)
                .context("Cannot create private credential file")?;
            let data = json!({"version":1,"router_api_key":key}).to_string();
            file.write_all(data.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&tmp, &self.file).context("Cannot replace local credential file")?;
            Ok(())
        })();
        let _ = fs::remove_file(&tmp);
        result
    }
    pub fn remove(&self) -> Result<()> {
        if self.check_file()? {
            fs::remove_file(&self.file).context("Cannot remove saved router key")?;
        }
        Ok(())
    }
}

pub fn secret_from_environment(value: &str) -> Result<Secret> {
    Ok(Secret(validate_key(value)?.into()))
}

pub fn router_key() -> Result<Option<Secret>> {
    match std::env::var(ROUTER_ENV) {
        Ok(value) if !value.trim().is_empty() => Ok(Some(Secret(validate_key(&value)?.into()))),
        _ => Store::default().load(),
    }
}

/// Import without putting the key in process arguments, shell history or chat.
/// Interactive terminals disable echo; piped input is supported as well.
pub fn import_from_terminal() -> Result<()> {
    use std::io::IsTerminal;
    if let Ok(key) = std::env::var(ROUTER_ENV) {
        return Store::default().save(&key);
    }
    let terminal = std::io::stdin().is_terminal();
    if terminal {
        eprint!("Router API key (hidden; stored locally): ");
        ratatui::crossterm::terminal::enable_raw_mode()?;
    }
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            if self.0 {
                let _ = ratatui::crossterm::terminal::disable_raw_mode();
                eprintln!();
            }
        }
    }
    let _restore = Restore(terminal);
    let mut key = String::new();
    if terminal {
        use ratatui::crossterm::event::{Event, KeyCode, KeyModifiers, read};
        loop {
            if let Event::Key(event) = read()? {
                match event.code {
                    KeyCode::Enter => break,
                    KeyCode::Esc => bail!("Key entry cancelled"),
                    KeyCode::Char('c') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                        bail!("Key entry cancelled")
                    }
                    KeyCode::Backspace => {
                        key.pop();
                    }
                    KeyCode::Char(c) if !event.modifiers.contains(KeyModifiers::CONTROL) => {
                        key.push(c);
                        if key.len() > MAX_KEY_BYTES {
                            bail!("Router API key is too long");
                        }
                    }
                    _ => {}
                }
            }
        }
    } else {
        std::io::stdin()
            .take(MAX_KEY_BYTES as u64 + 1)
            .read_to_string(&mut key)?;
    }
    Store::default().save(&key)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store(tag: &str) -> Store {
        static N: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "termed-key-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        Store::new(root.join("credentials.json"))
    }
    #[test]
    fn saved_key_round_trips_is_private_and_can_be_removed() {
        let s = store("roundtrip");
        assert!(s.load().unwrap().is_none());
        s.save("test-local-key").unwrap();
        let key = s.load().unwrap().unwrap();
        assert_eq!(key.expose(), "test-local-key");
        assert!(!format!("{key:?}").contains("test-local-key"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&s.file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        s.save("replacement").unwrap();
        assert_eq!(s.load().unwrap().unwrap().expose(), "replacement");
        assert!(s.save("bad\r\nheader").is_err());
        assert_eq!(s.load().unwrap().unwrap().expose(), "replacement");
        s.remove().unwrap();
        assert!(s.load().unwrap().is_none());
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_hardlinks_and_shared_permissions_are_refused() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let s = store("links");
        s.save("secret").unwrap();
        let alias = s.file.with_extension("link");
        symlink(&s.file, &alias).unwrap();
        assert!(Store::new(alias).save("no").is_err());
        fs::set_permissions(&s.file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(s.load().is_err());
        fs::set_permissions(&s.file, fs::Permissions::from_mode(0o600)).unwrap();
        fs::hard_link(&s.file, s.file.with_extension("hardlink")).unwrap();
        assert!(s.load().is_err());
    }
}
