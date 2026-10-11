use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::decisions::engine::DecisionEngine;
use crate::decisions::types::{ChoiceResult, DecisionContext, NoulResult, ScoreResult};

/// Serializable record for replay testing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionCassette {
    pub noul_records: HashMap<String, NoulResult>,
    pub choice_records: HashMap<String, ChoiceResult<String>>,
    pub score_records: HashMap<String, ScoreResult>,
}

impl Default for DecisionCassette {
    fn default() -> Self {
        Self {
            noul_records: HashMap::new(),
            choice_records: HashMap::new(),
            score_records: HashMap::new(),
        }
    }
}

/// Deterministic replay cassette backend for CI/CD and testing.
#[derive(Debug)]
pub struct CassetteBackend {
    cassette: Mutex<DecisionCassette>,
    recording: bool,
}

impl CassetteBackend {
    pub fn new_replay(cassette: DecisionCassette) -> Self {
        Self {
            cassette: Mutex::new(cassette),
            recording: false,
        }
    }

    pub fn new_recording() -> Self {
        Self {
            cassette: Mutex::new(DecisionCassette::default()),
            recording: true,
        }
    }

    pub fn export_cassette(&self) -> DecisionCassette {
        self.cassette.lock().unwrap().clone()
    }
}

impl DecisionEngine for CassetteBackend {
    fn backend_name(&self) -> &'static str {
        if self.recording { "cassette-recording" } else { "cassette-replay" }
    }

    fn ask_noul(&self, question: &str, _ctx: &DecisionContext) -> Result<NoulResult> {
        let mut lock = self.cassette.lock().unwrap();
        if self.recording {
            let res = NoulResult {
                judgment: true,
                confidence: 1.0,
                margin: 1.0,
                rationale: Some("Cassette recorded pass".into()),
            };
            lock.noul_records.insert(question.to_string(), res.clone());
            return Ok(res);
        }

        if let Some(recorded) = lock.noul_records.get(question) {
            Ok(recorded.clone())
        } else {
            // Default deterministic judgment if not explicitly in cassette
            Ok(NoulResult {
                judgment: true,
                confidence: 1.0,
                margin: 1.0,
                rationale: Some("Cassette unrecorded default pass".into()),
            })
        }
    }

    fn ask_choice(
        &self,
        question: &str,
        candidates: &[String],
        _ctx: &DecisionContext,
    ) -> Result<ChoiceResult<String>> {
        let mut lock = self.cassette.lock().unwrap();
        if self.recording && !candidates.is_empty() {
            let res = ChoiceResult {
                selected: candidates[0].clone(),
                index: 0,
                probabilities: vec![1.0],
                confidence: 1.0,
                rationale: Some("Cassette recorded selection".into()),
            };
            lock.choice_records.insert(question.to_string(), res.clone());
            return Ok(res);
        }

        if let Some(recorded) = lock.choice_records.get(question) {
            Ok(recorded.clone())
        } else if !candidates.is_empty() {
            Ok(ChoiceResult {
                selected: candidates[0].clone(),
                index: 0,
                probabilities: vec![1.0],
                confidence: 1.0,
                rationale: Some("Cassette unrecorded default selection".into()),
            })
        } else {
            anyhow::bail!("Cassette replay: empty candidates list");
        }
    }

    fn ask_score(
        &self,
        metric: &str,
        target_payload: &str,
        _ctx: &DecisionContext,
    ) -> Result<ScoreResult> {
        let key = format!("{}:{}", metric, target_payload);
        let mut lock = self.cassette.lock().unwrap();
        if self.recording {
            let res = ScoreResult {
                score: 0.10,
                uncertainty: 0.0,
                flags: vec!["cassette_recorded".into()],
                rationale: Some("Cassette recorded score".into()),
            };
            lock.score_records.insert(key, res.clone());
            return Ok(res);
        }

        if let Some(recorded) = lock.score_records.get(&key) {
            Ok(recorded.clone())
        } else {
            Ok(ScoreResult {
                score: 0.10,
                uncertainty: 0.0,
                flags: vec!["cassette_default".into()],
                rationale: Some("Cassette unrecorded default score".into()),
            })
        }
    }
}
