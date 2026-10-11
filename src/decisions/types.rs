use serde::{Deserialize, Serialize};

/// Context passed into typed decision queries.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionContext {
    pub task_id: String,
    pub working_dir: String,
    pub session_history_summary: String,
    pub metadata: serde_json::Value,
}

impl Default for DecisionContext {
    fn default() -> Self {
        Self {
            task_id: "default_task".into(),
            working_dir: ".".into(),
            session_history_summary: String::new(),
            metadata: serde_json::Value::Null,
        }
    }
}

/// A `noul` decision: binary judgment (true/false) with confidence and margin.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoulResult {
    pub judgment: bool,
    /// Calibrated confidence score between 0.0 and 1.0.
    pub confidence: f32,
    /// Difference between top probability and decision threshold.
    pub margin: f32,
    pub rationale: Option<String>,
}

/// A `choice` decision: selection from a closed discrete set of options.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChoiceResult<T: Clone> {
    pub selected: T,
    pub index: usize,
    pub probabilities: Vec<f32>,
    pub confidence: f32,
    pub rationale: Option<String>,
}

/// A `score` decision: continuous calibrated metric between 0.0 and 1.0.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoreResult {
    /// Risk or metric value between 0.0 (safe/low) and 1.0 (dangerous/high).
    pub score: f32,
    /// Epistemic uncertainty between 0.0 and 1.0.
    pub uncertainty: f32,
    pub flags: Vec<String>,
    pub rationale: Option<String>,
}
