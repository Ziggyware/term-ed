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

use std::sync::{Arc, OnceLock};

/// Default decision engine instance based on environment configuration.
/// Defaults to LocalHeuristicBackend (zero config, free, offline).
/// Uses JevHttpBackend if `TERMED_JEV_API_KEY` is set.
///
/// Process-wide singleton: both backends are stateless after construction
/// (the heuristic backend's fields are three immutable `Vec<Regex>` read
/// only through `&self`; the Jev backend's fields are immutable strings,
/// a pooled HTTP agent, and that same heuristic fallback), and
/// `DecisionEngine` requires `Send + Sync` — so one instance is safely
/// shared by every worker instead of recompiling the regex set (and, for
/// Jev, rebuilding the HTTP agent) per worker. The environment is read
/// once, on first use.
pub fn default_engine() -> Arc<dyn DecisionEngine> {
    static ENGINE: OnceLock<Arc<dyn DecisionEngine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            if let Ok(key) = std::env::var("TERMED_JEV_API_KEY") {
                if !key.trim().is_empty() {
                    let url = std::env::var("TERMED_JEV_URL")
                        .unwrap_or_else(|_| "https://api.typesafe.ai/v1".into());
                    return Arc::new(JevHttpBackend::new(url, key));
                }
            }
            Arc::new(LocalHeuristicBackend::new())
        })
        .clone()
}
