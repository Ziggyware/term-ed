//! Std-only performance probe for the optimization pass. Times the
//! operations the scouts flagged as hot, on this repo as the fixture.
//! Run: `cargo run --release --example perf_probe` (median of repeats
//! is printed per operation). Not a benchmark suite — a before/after
//! proof harness: the same numbers must move down, never up.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use termed::config::AgentConfig;

fn median_ms(mut runs: Vec<f64>) -> f64 {
    runs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    runs[runs.len() / 2]
}

fn main() {
    // PROBE_ROOT pins the search/code-intel fixture so before/after runs
    // walk an identical corpus (the repo's own target/ dir grows between
    // builds and would contaminate the comparison).
    let root = std::env::var_os("PROBE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));

    // 1. Tool schema construction (rebuilt per agent step today).
    let mut options = AgentConfig::default();
    options.plugins = true;
    options.sub_agents = true;
    options.generated_tools = true;
    let runs: Vec<f64> = (0..7)
        .map(|_| {
            let t = Instant::now();
            for _ in 0..100 {
                let tools = termed::tool_bridge::tools(&options);
                std::hint::black_box(tools.len());
            }
            t.elapsed().as_secs_f64() * 1000.0 / 100.0
        })
        .collect();
    println!("tools_schema_build_ms_per_call: {:.3}", median_ms(runs));

    // 2. Project search over this repo: one rare and one common literal.
    for needle in ["salvage_write", "fn "] {
        let runs: Vec<f64> = (0..5)
            .map(|_| {
                let q = termed::search::Query::new(needle);
                let filter = termed::search::FilterSet::default();
                let overrides: HashMap<PathBuf, String> = HashMap::new();
                let t = Instant::now();
                let results = termed::search::search(&root, &q, &filter, &overrides)
                    .expect("search runs");
                std::hint::black_box(results.matches);
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        println!("search_{needle:?}_ms: {:.1}", median_ms(runs));
    }

    // 3. Code intelligence over the largest source file.
    let big = std::fs::read_to_string(root.join("src/builtin_agent.rs")).unwrap();
    for (name, f) in [
        ("occurrences", termed::code_intel::occurrences as fn(&str, &str) -> Vec<termed::code_intel::Occurrence>),
        ("references", termed::code_intel::references as fn(&str, &str) -> Vec<termed::code_intel::Occurrence>),
    ] {
        let runs: Vec<f64> = (0..5)
            .map(|_| {
                let t = Instant::now();
                let out = f(&big, "history");
                std::hint::black_box(out.len());
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        println!("code_intel_{name}_ms: {:.2}", median_ms(runs));
    }
}
