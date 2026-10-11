use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

/// Single hunk of an Agent Editing Protocol modification.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AepHunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<String>,
}

/// A structured patch affecting a specific file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AepPatch {
    pub path: String,
    pub old_text: String,
    pub new_text: String,
    pub hunks: Vec<AepHunk>,
    pub created_at_ms: u64,
}

impl AepPatch {
    pub fn from_replace(path: impl Into<String>, old_text: impl Into<String>, new_text: impl Into<String>) -> Self {
        let path = path.into();
        let old_text = old_text.into();
        let new_text = new_text.into();

        let diff = TextDiff::from_lines(&old_text, &new_text);
        let mut hunks = Vec::new();

        for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
            let mut lines = Vec::new();
            for change in hunk.iter_changes() {
                let prefix = match change.tag() {
                    ChangeTag::Delete => "-",
                    ChangeTag::Insert => "+",
                    ChangeTag::Equal => " ",
                };
                lines.push(format!("{}{}", prefix, change.value()));
            }
            hunks.push(AepHunk {
                old_start: hunk.old_range().start(),
                old_lines: hunk.old_range().len(),
                new_start: hunk.new_range().start(),
                new_lines: hunk.new_range().len(),
                lines,
            });
        }

        Self {
            path,
            old_text,
            new_text,
            hunks,
            created_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }
}

/// Applies an exact replacement patch to a target document string.
pub fn apply_patch_to_string(content: &str, old_text: &str, new_text: &str) -> Result<String, String> {
    let count = content.matches(old_text).count();
    if count == 0 {
        return Err("old_text not found in file".into());
    }
    if count > 1 {
        return Err(format!("old_text matches {count} times; provide more context to make it unique"));
    }
    Ok(content.replacen(old_text, new_text, 1))
}

/// Generates a standardized unified diff string for display in UI or logs.
pub fn generate_unified_diff(path: &str, old_text: &str, new_text: &str) -> String {
    let diff = TextDiff::from_lines(old_text, new_text);
    diff.unified_diff()
        .header(&format!("a/{}", path), &format!("b/{}", path))
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aep_patch_and_replacement() {
        let content = "def add(a, b):\n    return a - b\n";
        let patched = apply_patch_to_string(content, "return a - b", "return a + b").unwrap();
        assert_eq!(patched, "def add(a, b):\n    return a + b\n");

        let err = apply_patch_to_string(content, "non_existent", "foo");
        assert!(err.is_err());

        let patch = AepPatch::from_replace("calc.py", "return a - b", "return a + b");
        assert_eq!(patch.path, "calc.py");
        assert!(!patch.hunks.is_empty());
    }
}
