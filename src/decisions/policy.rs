use std::collections::HashSet;
use std::sync::Arc;

use serde_json::json;

use crate::decisions::engine::DecisionEngine;
use crate::decisions::types::{DecisionContext, ScoreResult};

/// Outcome of evaluating an operation against the PermissionPolicy.
#[derive(Clone, Debug, PartialEq)]
pub enum PermissionVerdict {
    /// Safe to execute immediately without interrupting the user.
    AutoAllow { score: f32, rationale: String },
    /// Requires explicit confirmation by the user (interactive modal/prompt).
    PromptUser { score: f32, rationale: String },
    /// Forbidden due to high destructive potential or security policy.
    Block { score: f32, reason: String },
}

/// Dynamic permission policy driven by typed risk scores.
#[derive(Clone, Debug)]
pub struct PermissionPolicy {
    engine: Arc<dyn DecisionEngine>,
    auto_allow_threshold: f32, // default: 0.20
    block_threshold: f32,      // default: 0.75
}

impl PermissionPolicy {
    pub fn new(engine: Arc<dyn DecisionEngine>) -> Self {
        Self {
            engine,
            auto_allow_threshold: 0.20,
            block_threshold: 0.75,
        }
    }

    pub fn with_thresholds(
        engine: Arc<dyn DecisionEngine>,
        auto_allow_threshold: f32,
        block_threshold: f32,
    ) -> Self {
        Self {
            engine,
            auto_allow_threshold,
            block_threshold,
        }
    }

    /// Evaluates a shell command and returns the tiered permission verdict.
    pub fn evaluate_command(&self, command: &str, cwd: &str) -> PermissionVerdict {
        let ctx = DecisionContext {
            task_id: "shell_permission_check".into(),
            working_dir: cwd.into(),
            session_history_summary: String::new(),
            metadata: json!({ "command": command }),
        };

        let result = self
            .engine
            .ask_score("shell_execution_risk", command, &ctx)
            .unwrap_or(ScoreResult {
                score: 0.50,
                uncertainty: 0.50,
                flags: vec!["policy_fallback".into()],
                rationale: Some("Fallback policy applied".into()),
            });

        let rationale = result
            .rationale
            .unwrap_or_else(|| format!("Risk score: {:.2}", result.score));

        if result.score < self.auto_allow_threshold {
            PermissionVerdict::AutoAllow {
                score: result.score,
                rationale,
            }
        } else if result.score >= self.block_threshold {
            PermissionVerdict::Block {
                score: result.score,
                reason: format!("Execution blocked: {} (risk: {:.2})", rationale, result.score),
            }
        } else {
            PermissionVerdict::PromptUser {
                score: result.score,
                rationale,
            }
        }
    }

    /// Evaluates source code about to be executed by run_code and returns
    /// the tiered permission verdict. Code gets its own metric
    /// (`code_execution_risk`): scoring a script with the shell-command
    /// signatures matched nothing and landed every script in the generic
    /// 0.45 "unrecognized" prompt tier. The code metric reads the script
    /// itself — network egress, process spawning, credential access,
    /// destructive file operations, and paths outside the workspace raise
    /// the score; plain computation and workspace file I/O stay low.
    /// Same thresholds as commands and writes.
    pub fn evaluate_code(&self, code: &str, language: &str, cwd: &str) -> PermissionVerdict {
        let ctx = DecisionContext {
            task_id: "code_permission_check".into(),
            working_dir: cwd.into(),
            session_history_summary: String::new(),
            metadata: json!({ "language": language }),
        };

        let result = self
            .engine
            .ask_score("code_execution_risk", code, &ctx)
            .unwrap_or(ScoreResult {
                score: 0.50,
                uncertainty: 0.50,
                flags: vec!["policy_fallback".into()],
                rationale: Some("Fallback policy applied".into()),
            });

        let rationale = result
            .rationale
            .unwrap_or_else(|| format!("Risk score: {:.2}", result.score));

        if result.score < self.auto_allow_threshold {
            PermissionVerdict::AutoAllow {
                score: result.score,
                rationale,
            }
        } else if result.score >= self.block_threshold {
            PermissionVerdict::Block {
                score: result.score,
                reason: format!("Execution blocked: {} (risk: {:.2})", rationale, result.score),
            }
        } else {
            PermissionVerdict::PromptUser {
                score: result.score,
                rationale,
            }
        }
    }

    /// Evaluates a file write (a "write order" from the main agent or a
    /// sub-agent) and returns the tiered permission verdict. Same thresholds
    /// as commands: ordinary source files auto-allow, build/CI configuration
    /// prompts, and sensitive paths (secrets, git internals) block.
    pub fn evaluate_write(&self, path: &str, kind: &str, cwd: &str) -> PermissionVerdict {
        let ctx = DecisionContext {
            task_id: "file_write_check".into(),
            working_dir: cwd.into(),
            session_history_summary: String::new(),
            metadata: json!({ "path": path, "kind": kind }),
        };

        let result = self
            .engine
            .ask_score("file_write_risk", path, &ctx)
            .unwrap_or(ScoreResult {
                score: 0.50,
                uncertainty: 0.50,
                flags: vec!["policy_fallback".into()],
                rationale: Some("Fallback policy applied".into()),
            });

        let rationale = result
            .rationale
            .unwrap_or_else(|| format!("Risk score: {:.2}", result.score));

        if result.score < self.auto_allow_threshold {
            PermissionVerdict::AutoAllow {
                score: result.score,
                rationale,
            }
        } else if result.score >= self.block_threshold {
            PermissionVerdict::Block {
                score: result.score,
                reason: format!(
                    "Write blocked: {} (risk: {:.2})",
                    rationale, result.score
                ),
            }
        } else {
            PermissionVerdict::PromptUser {
                score: result.score,
                rationale,
            }
        }
    }
}

/// Intelligent model selection and failover using Jev choice.
#[derive(Clone, Debug)]
pub struct ModelSelector {
    engine: Arc<dyn DecisionEngine>,
}

impl ModelSelector {
    pub fn new(engine: Arc<dyn DecisionEngine>) -> Self {
        Self { engine }
    }

    /// Chooses the best alternative model when the active model stalls or fails.
    pub fn select_fallback(
        &self,
        current_model: &str,
        candidates: &[(String, String)],
        busy_models: &HashSet<String>,
    ) -> Option<String> {
        // Filter out current model and known busy models
        let pool: Vec<String> = candidates
            .iter()
            .map(|(m, _)| m.clone())
            .filter(|m| m != current_model && !busy_models.contains(m))
            .collect();

        if pool.is_empty() {
            return None;
        }

        let ctx = DecisionContext {
            task_id: "model_failover_choice".into(),
            working_dir: ".".into(),
            session_history_summary: format!("Current failed model: {current_model}"),
            metadata: json!({
                "current_model": current_model,
                "busy_models": busy_models.iter().collect::<Vec<_>>()
            }),
        };

        match self.engine.ask_choice("Select replacement model", &pool, &ctx) {
            Ok(res) => Some(res.selected),
            Err(_) => pool.first().cloned(),
        }
    }
}
