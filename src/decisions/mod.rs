//! Typed decision engine for term-ed.
//!
//! Provides structured probabilistic decisions (`noul`, `choice`, `score`)
//! replacing brittle LLM prompt parsing. Includes a local offline heuristic
//! backend, a TypeSafe Jev HTTP backend, and a replay cassette backend.

pub mod cassette;
pub mod engine;
pub mod heuristic;
pub mod jev;
pub mod policy;
pub mod types;

pub use cassette::CassetteBackend;
pub use engine::DecisionEngine;
pub use heuristic::LocalHeuristicBackend;
pub use jev::JevHttpBackend;
pub use policy::{ModelSelector, PermissionPolicy, PermissionVerdict};
pub use types::{ChoiceResult, DecisionContext, NoulResult, ScoreResult};

use std::sync::Arc;

/// Default decision engine instance based on environment configuration.
/// Defaults to LocalHeuristicBackend (zero config, free, offline).
/// Uses JevHttpBackend if `TERMED_JEV_API_KEY` is set.
pub fn default_engine() -> Arc<dyn DecisionEngine> {
    if let Ok(key) = std::env::var("TERMED_JEV_API_KEY") {
        if !key.trim().is_empty() {
            let url = std::env::var("TERMED_JEV_URL")
                .unwrap_or_else(|_| "https://api.typesafe.ai/v1".into());
            return Arc::new(JevHttpBackend::new(url, key));
        }
    }
    Arc::new(LocalHeuristicBackend::new())
}
