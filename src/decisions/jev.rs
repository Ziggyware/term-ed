use anyhow::Result;
use serde_json::json;
use std::time::Duration;

use crate::decisions::engine::DecisionEngine;
use crate::decisions::heuristic::LocalHeuristicBackend;
use crate::decisions::types::{ChoiceResult, DecisionContext, NoulResult, ScoreResult};

/// TypeSafe Jev Decision Engine client.
/// Communicates over HTTP with the Jev API to obtain calibrated probabilistic decisions.
/// Falls back transparently to LocalHeuristicBackend if the network or endpoint is unavailable.
#[derive(Debug)]
pub struct JevHttpBackend {
    endpoint: String,
    api_key: String,
    fallback: LocalHeuristicBackend,
}

impl JevHttpBackend {
    pub fn new(endpoint: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            fallback: LocalHeuristicBackend::new(),
        }
    }

    fn client(&self) -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(4)))
            .build()
            .new_agent()
    }
}

impl DecisionEngine for JevHttpBackend {
    fn backend_name(&self) -> &'static str {
        "jev-typesafe-cloud"
    }

    fn ask_noul(&self, question: &str, ctx: &DecisionContext) -> Result<NoulResult> {
        let body = json!({
            "primitive": "noul",
            "question": question,
            "context": {
                "task_id": ctx.task_id,
                "working_dir": ctx.working_dir,
                "metadata": ctx.metadata
            }
        });

        let url = format!("{}/decisions/noul", self.endpoint);
        let resp = self.client()
            .post(&url)
            .header("Authorization", &format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("User-Agent", "termed-jev-engine/1.0")
            .send(body.to_string());

        if let Ok(res) = resp {
            if res.status().as_u16() == 200 {
                let text = res.into_body().read_to_string().unwrap_or_default();
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let (Some(judgment), Some(confidence)) = (
                        parsed.get("judgment").and_then(|v| v.as_bool()),
                        parsed.get("confidence").and_then(|v| v.as_f64()),
                    ) {
                        return Ok(NoulResult {
                            judgment,
                            confidence: confidence as f32,
                            margin: parsed.get("margin").and_then(|v| v.as_f64()).unwrap_or(0.5) as f32,
                            rationale: parsed.get("rationale").and_then(|v| v.as_str()).map(String::from),
                        });
                    }
                }
            }
        }

        // Graceful fallback to local heuristic
        self.fallback.ask_noul(question, ctx)
    }

    fn ask_choice(
        &self,
        question: &str,
        candidates: &[String],
        ctx: &DecisionContext,
    ) -> Result<ChoiceResult<String>> {
        let body = json!({
            "primitive": "choice",
            "question": question,
            "candidates": candidates,
            "context": {
                "task_id": ctx.task_id,
                "metadata": ctx.metadata
            }
        });

        let url = format!("{}/decisions/choice", self.endpoint);
        let resp = self.client()
            .post(&url)
            .header("Authorization", &format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("User-Agent", "termed-jev-engine/1.0")
            .send(body.to_string());

        if let Ok(res) = resp {
            if res.status().as_u16() == 200 {
                let text = res.into_body().read_to_string().unwrap_or_default();
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let (Some(selected), Some(index)) = (
                        parsed.get("selected").and_then(|v| v.as_str()),
                        parsed.get("index").and_then(|v| v.as_u64()),
                    ) {
                        let probs = parsed.get("probabilities")
                            .and_then(|v| v.as_array())
                            .map(|arr| arr.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect())
                            .unwrap_or_default();
                        let conf = parsed.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.8) as f32;
                        return Ok(ChoiceResult {
                            selected: selected.to_string(),
                            index: index as usize,
                            probabilities: probs,
                            confidence: conf,
                            rationale: parsed.get("rationale").and_then(|v| v.as_str()).map(String::from),
                        });
                    }
                }
            }
        }

        // Graceful fallback to local heuristic
        self.fallback.ask_choice(question, candidates, ctx)
    }

    fn ask_score(
        &self,
        metric: &str,
        target_payload: &str,
        ctx: &DecisionContext,
    ) -> Result<ScoreResult> {
        let body = json!({
            "primitive": "score",
            "metric": metric,
            "target": target_payload,
            "context": {
                "task_id": ctx.task_id,
                "metadata": ctx.metadata
            }
        });

        let url = format!("{}/decisions/score", self.endpoint);
        let resp = self.client()
            .post(&url)
            .header("Authorization", &format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("User-Agent", "termed-jev-engine/1.0")
            .send(body.to_string());

        if let Ok(res) = resp {
            if res.status().as_u16() == 200 {
                let text = res.into_body().read_to_string().unwrap_or_default();
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(score) = parsed.get("score").and_then(|v| v.as_f64()) {
                        let uncertainty = parsed.get("uncertainty").and_then(|v| v.as_f64()).unwrap_or(0.1) as f32;
                        let flags = parsed.get("flags")
                            .and_then(|v| v.as_array())
                            .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                            .unwrap_or_default();
                        return Ok(ScoreResult {
                            score: score as f32,
                            uncertainty,
                            flags,
                            rationale: parsed.get("rationale").and_then(|v| v.as_str()).map(String::from),
                        });
                    }
                }
            }
        }

        // Graceful fallback to local heuristic
        self.fallback.ask_score(metric, target_payload, ctx)
    }
}
