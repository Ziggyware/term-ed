//! Local instruction skills. These are Markdown, not executable plugins.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use crate::config::{config_dir, valid_id};

pub const MAX_SKILL_BYTES: u64 = 32 * 1024;

#[derive(Clone, Debug)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub enabled: bool,
    pub workspace: bool,
}

impl Skill {
    pub fn read(&self) -> Result<String> {
        if crate::credentials::is_private_path(&self.path) {
            bail!("Credential files cannot be used as instruction skills");
        }
        let meta = std::fs::metadata(&self.path)?;
        if meta.len() > MAX_SKILL_BYTES {
            bail!("Skill {} exceeds 32 KB", self.id);
        }
        std::fs::read_to_string(&self.path).context("Cannot read skill instructions")
    }
    pub fn metadata(&self) -> Value {
        json!({"id": self.id, "name": self.name, "description": self.description})
    }
}

pub fn workspace_dir(root: &Path) -> PathBuf {
    root.join(".termed/skills")
}

pub fn discover(root: &Path, disabled: &[String]) -> Vec<Skill> {
    let mut found = std::collections::BTreeMap::new();
    for (dir, workspace) in [
        (config_dir().join("skills"), false),
        (workspace_dir(root), true),
    ] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            let Some(id) = path
                .file_name()
                .and_then(|s| s.to_str())
                .filter(|s| valid_id(s))
            else {
                continue;
            };
            let file = path.join("SKILL.md");
            // Do not follow skill symlinks out of their configured skill directory.
            let Ok(canonical) = file.canonicalize() else {
                continue;
            };
            let Ok(base) = dir.canonicalize() else {
                continue;
            };
            if !canonical.starts_with(base) {
                continue;
            }
            let mut skill = Skill {
                id: id.into(),
                name: id.replace('-', " "),
                description: String::new(),
                // Match the editor's canonical path identity so alias paths
                // cannot bypass dirty-buffer protection during deletion.
                path: canonical,
                enabled: !disabled.iter().any(|s| s == id),
                workspace,
            };
            if let Ok(text) = skill.read() {
                let mut frontmatter = false;
                for (i, line) in text.lines().enumerate() {
                    if i == 0 && line.trim() == "---" {
                        frontmatter = true;
                        continue;
                    }
                    if frontmatter {
                        if line.trim() == "---" {
                            frontmatter = false;
                            continue;
                        }
                        if let Some(name) = line.strip_prefix("name:") {
                            skill.name = name.trim().trim_matches(['\'', '"']).to_string();
                        }
                        if let Some(desc) = line.strip_prefix("description:") {
                            skill.description = desc.trim().trim_matches(['\'', '"']).to_string();
                        }
                    } else if let Some(name) = line.strip_prefix("# ") {
                        if skill.name == id.replace('-', " ") {
                            skill.name = name.trim().into();
                        }
                    } else if skill.description.is_empty() && !line.trim().is_empty() {
                        skill.description = line.chars().take(140).collect();
                    }
                }
            } else {
                skill.description =
                    "Cannot read skill (missing, invalid UTF-8 or over 32 KB)".into();
            }
            found.insert(id.to_string(), skill);
            if found.len() >= 128 {
                break;
            }
        }
    }
    found.into_values().collect()
}

pub fn create(root: &Path, id: &str) -> Result<PathBuf> {
    if !valid_id(id) {
        bail!("Use 1–64 letters, digits, '-' or '_' for the skill ID");
    }
    let dir = workspace_dir(root).join(id);
    let file = dir.join("SKILL.md");
    // create_new prevents overwriting existing user instructions.
    std::fs::create_dir_all(&dir)?;
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)?;
    use std::io::Write;
    write!(
        out,
        "---\nname: {id}\ndescription: Describe when the AI should use this skill\n---\n\n# {id}\n\nAdd instructions, conventions and verification steps here.\n"
    )?;
    Ok(file)
}

pub fn remove(root: &Path, skill: &Skill) -> Result<()> {
    if !skill.workspace {
        bail!(
            "Global skills can be disabled here; remove them from the user skills folder manually"
        );
    }
    let base = workspace_dir(root).canonicalize()?;
    if !skill.path.canonicalize()?.starts_with(base) {
        bail!("Skill is outside the workspace skills folder");
    }
    // Remove the instruction file only, never arbitrary files in the folder.
    std::fs::remove_file(&skill.path)?;
    if let Some(dir) = skill.path.parent() {
        let _ = std::fs::remove_dir(dir);
    }
    Ok(())
}

pub fn context(skills: &[Skill], selected: &[String]) -> String {
    let mut text = String::new();
    for skill in skills
        .iter()
        .filter(|s| s.enabled && selected.contains(&s.id))
    {
        if let Ok(body) = skill.read() {
            if text.len() + body.len() > 96 * 1024 {
                break;
            }
            text.push_str(&format!("\n\nSkill {}:\n{}", skill.id, body));
        }
    }
    text
}

/// Required instructions are never silently skipped or truncated. Callers keep
/// inherited skill snapshots and use this only for newly attached role skills.
pub fn required_context(skills: &[Skill], selected: &[String]) -> Result<String> {
    let mut context = String::new();
    let mut seen = std::collections::HashSet::new();
    for id in selected {
        if !seen.insert(id) {
            continue;
        }
        let skill = skills
            .iter()
            .find(|s| s.enabled && &s.id == id)
            .with_context(|| format!("Required skill {id} is missing or disabled"))?;
        let body = skill
            .read()
            .with_context(|| format!("Cannot load required skill {id}"))?;
        let block = format!("\n\nSkill {id}:\n{body}");
        if context.len() + block.len() > 96 * 1024 {
            bail!(
                "Required skills exceed 96 KiB; select fewer skills rather than omitting instructions"
            );
        }
        context.push_str(&block);
    }
    Ok(context)
}
