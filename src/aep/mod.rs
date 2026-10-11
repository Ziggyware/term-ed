//! Agent Editing Protocol (AEP) for term-ed.
//!
//! Standardizes structured multi-file diffs, fuzzy hunk application against
//! editor text buffers, and atomic reversible transactions.

pub mod diff;

pub use diff::{AepHunk, AepPatch, apply_patch_to_string, generate_unified_diff};
