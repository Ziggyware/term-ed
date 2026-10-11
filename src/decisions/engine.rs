use anyhow::Result;
use std::fmt::Debug;

use crate::decisions::types::{ChoiceResult, DecisionContext, NoulResult, ScoreResult};

/// Core trait representing a typed decision engine.
pub trait DecisionEngine: Send + Sync + Debug {
    /// Evaluate a binary proposition (True / False).
    fn ask_noul(&self, question: &str, ctx: &DecisionContext) -> Result<NoulResult>;

    /// Select the best option from a discrete closed candidate list.
    fn ask_choice(
        &self,
        question: &str,
        candidates: &[String],
        ctx: &DecisionContext,
    ) -> Result<ChoiceResult<String>>;

    /// Evaluate a continuous calibrated metric between 0.0 and 1.0.
    fn ask_score(
        &self,
        metric: &str,
        target_payload: &str,
        ctx: &DecisionContext,
    ) -> Result<ScoreResult>;

    /// Descriptive identifier of the active backend.
    fn backend_name(&self) -> &'static str;
}
