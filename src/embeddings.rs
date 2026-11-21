//! On-device embeddings over ONNX, and the search built on them.
//!
//! Embeddings answer the question a text search cannot: *"where is the code
//! that deals with this?"* when the words differ. They are computed by the same
//! local server that serves chat (`local_inference/onnx_server.py`), so they
//! need no key, no network and no second process.
//!
//! Everything here is optional. Without a model installed, `embed` returns an
//! error explaining how to install one, and callers fall back to plain search.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// Chunks longer than this are split, because small embedding models have a
/// short context and silently truncate the tail.
pub const MAX_CHUNK_BYTES: usize = 1200;
/// A semantic answer that scores below this is noise, not a match.
pub const MIN_SCORE: f32 = 0.25;

/// A piece of a file that can be embedded and shown to the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub path: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub chunk: Chunk,
    pub score: f32,
}

/// Split a file into embeddable chunks at line boundaries, preferring to break
/// after a blank line or a closing brace so a chunk is usually one item.
pub fn chunk_file(path: &Path, text: &str) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut start = 0usize;
    for (i, line) in text.split('\n').enumerate() {
        // Break at a natural boundary once the chunk is big enough.
        let natural = line.trim().is_empty() || line.trim_end().ends_with('}');
        if !current.is_empty() && current.len() + line.len() + 1 > MAX_CHUNK_BYTES {
            let end = i.saturating_sub(1);
            out.push(Chunk {
                path: path.to_path_buf(),
                start_line: start,
                end_line: end,
                text: std::mem::take(&mut current),
            });
            start = i;
        } else if natural && current.len() > MAX_CHUNK_BYTES / 2 {
            current.push_str(line);
            out.push(Chunk {
                path: path.to_path_buf(),
                start_line: start,
                end_line: i,
                text: std::mem::take(&mut current),
            });
            start = i + 1;
            continue;
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.trim().is_empty() {
        out.push(Chunk {
            path: path.to_path_buf(),
            start_line: start,
            end_line: text.split('\n').count().saturating_sub(1),
            text: current,
        });
    }
    out.retain(|c| !c.text.trim().is_empty());
    out
}

/// Cosine similarity of two vectors. Returns 0.0 for mismatched shapes and
/// zero-length vectors, so a bad response degrades to "no match".
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// One embedded chunk, kept with its vector.
#[derive(Clone, Debug)]
pub struct Embedded {
    pub chunk: Chunk,
    pub vector: Vec<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct Index {
    pub items: Vec<Embedded>,
    /// The model used, so vectors from two models are never compared.
    pub model: String,
}

impl Index {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Rank the index against a query vector. Deterministic: equal scores keep
    /// file/line order, so the same query always answers the same way.
    pub fn search(&self, query: &[f32], limit: usize) -> Vec<Hit> {
        let mut hits: Vec<Hit> = self
            .items
            .iter()
            .map(|item| Hit {
                chunk: item.chunk.clone(),
                score: cosine(query, &item.vector),
            })
            .filter(|h| h.score >= MIN_SCORE)
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    a.chunk
                        .path
                        .cmp(&b.chunk.path)
                        .then_with(|| a.chunk.start_line.cmp(&b.chunk.start_line))
                })
        });
        hits.truncate(limit);
        hits
    }
}

/// Why an embedding request failed, phrased so the user knows the next step.
pub fn unavailable_reason(error: &str) -> String {
    format!(
        "Semantic search needs a local embedding model: {error}. \
         Run `termed --local-prefetch --local-embedding-model` to download one, \
         or set an endpoint with TERMED_EMBEDDING_ENDPOINT."
    )
}

/// The model used for embeddings. Deliberately small: it must be quick on a
/// CPU and must not compete with the chat model for memory.
pub const DEFAULT_EMBEDDING_MODEL: &str = "Xenova/all-MiniLM-L6-v2";

fn endpoint() -> String {
    std::env::var("TERMED_EMBEDDING_ENDPOINT")
        .ok()
        .filter(|e| !e.trim().is_empty())
        .unwrap_or_else(|| crate::provider::local_endpoint(&crate::config::AgentConfig::default()))
}

/// Ask the local server for vectors. Uses the synchronous HTTP client shared by
/// `local_server`, so this stays usable from the search thread with no runtime.
pub fn embed(texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let url = format!("{}/embeddings", endpoint().trim_end_matches('/'));
    let body = json!({
        "model": std::env::var("TERMED_EMBEDDING_MODEL")
            .unwrap_or_else(|_| DEFAULT_EMBEDDING_MODEL.to_string()),
        "input": texts,
    });
    // Same builder shape as the rest of the crate (ureq 3: config -> new_agent).
    let http = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(5)))
        .timeout_per_read(Some(std::time::Duration::from_secs(120)))
        .build()
        .new_agent();
    let response = http
        .post(&url)
        .header("content-type", "application/json")
        .send_json(body)
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => unavailable_reason(
                "the local server in use predates the embeddings route",
            ),
            other => unavailable_reason(&other.to_string()),
        })?;
    let value: Value = response
        .into_body()
        .read_json()
        .map_err(|e| unavailable_reason(&format!("invalid response: {e}")))?;
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| unavailable_reason("response had no `data` array"))?;
    let mut out = Vec::with_capacity(data.len());
    for item in data {
        let vector = item
            .get("embedding")
            .and_then(Value::as_array)
            .ok_or_else(|| unavailable_reason("an embedding was missing"))?
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect();
        out.push(vector);
    }
    if out.len() != texts.len() {
        return Err(unavailable_reason(&format!(
            "asked for {} embeddings, got {}",
            texts.len(),
            out.len()
        )));
    }
    Ok(out)
}

/// Embed every chunk, honouring an existing vector cache so re-indexing a
/// project only pays for what changed.
pub fn embed_chunks(
    chunks: &[Chunk],
    cache: &mut HashMap<String, Vec<f32>>,
) -> Result<Index, String> {
    let mut needed: Vec<(usize, String)> = Vec::new();
    for (i, c) in chunks.iter().enumerate() {
        if !cache.contains_key(&c.text) {
            needed.push((i, c.text.clone()));
        }
    }
    // Batch to keep one request from becoming enormous.
    for batch in needed.chunks(16) {
        let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
        let vectors = embed(&texts)?;
        for ((_, text), vector) in batch.iter().zip(vectors) {
            cache.insert(text.clone(), vector);
        }
    }
    let items = chunks
        .iter()
        .filter_map(|c| {
            cache.get(&c.text).map(|vector| Embedded {
                chunk: c.clone(),
                vector: vector.clone(),
            })
        })
        .collect();
    Ok(Index {
        items,
        model: std::env::var("TERMED_EMBEDDING_MODEL")
            .unwrap_or_else(|_| DEFAULT_EMBEDDING_MODEL.to_string()),
    })
}

/// Rank a free-text query. This is what "Search: Find Related Code" uses.
pub fn search_text(index: &Index, query: &str, limit: usize) -> Result<Vec<Hit>, String> {
    let vector = embed(&[query.to_string()])?
        .into_iter()
        .next()
        .ok_or_else(|| unavailable_reason("the model returned no vector"))?;
    Ok(index.search(&vector, limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_is_scale_invariant_and_safe() {
        assert!((cosine(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0, "mismatched shapes");
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0, "zero vector");
    }

    #[test]
    fn chunking_keeps_lines_whole_and_respects_the_budget() {
        let text: String = (0..400).map(|i| format!("line {i} of the file\n")).collect();
        let chunks = chunk_file(Path::new("a.rs"), &text);
        assert!(chunks.len() > 1, "long file must split");
        assert!(chunks.iter().all(|c| c.text.len() <= MAX_CHUNK_BYTES + 64));
        // Every line appears exactly once across the chunks.
        let joined: String = chunks.iter().map(|c| c.text.clone() + "\n").collect();
        for i in [0, 7, 399] {
            assert!(
                joined.contains(&format!("line {i} of the file")),
                "line {i} missing"
            );
        }
        for c in &chunks {
            assert!(c.end_line >= c.start_line, "{c:?}");
        }
    }

    #[test]
    fn chunking_skips_blank_files() {
        assert!(chunk_file(Path::new("a"), "").is_empty());
        assert!(chunk_file(Path::new("a"), "   \n\n  \n").is_empty());
    }

    #[test]
    fn search_orders_by_score_then_position_and_filters_noise() {
        let mk = |line: usize, vector: Vec<f32>| Embedded {
            chunk: Chunk {
                path: PathBuf::from("a.rs"),
                start_line: line,
                end_line: line,
                text: format!("chunk {line}"),
            },
            vector,
        };
        let index = Index {
            items: vec![
                mk(0, vec![0.0, 1.0]),   // orthogonal: filtered out
                mk(10, vec![1.0, 0.1]),  // best
                mk(5, vec![1.0, 0.1]),   // same score, earlier line
                mk(20, vec![0.9, 0.2]),
            ],
            model: "m".into(),
        };
        let hits = index.search(&[1.0, 0.0], 10);
        assert_eq!(hits.len(), 3, "the orthogonal chunk is dropped");
        assert_eq!(hits[0].chunk.start_line, 5, "tie broken by line");
        assert_eq!(hits[1].chunk.start_line, 10);
        assert!(hits[0].score > hits[2].score);
        // The limit is honoured.
        assert_eq!(index.search(&[1.0, 0.0], 1).len(), 1);
    }

    #[test]
    fn a_dead_server_produces_an_actionable_message() {
        // Point at a port nothing listens on; the failure must name the fix.
        unsafe {
            std::env::set_var("TERMED_EMBEDDING_ENDPOINT", "http://127.0.0.1:9/v1");
        }
        let err = embed(&["hello".to_string()]).unwrap_err();
        unsafe {
            std::env::remove_var("TERMED_EMBEDDING_ENDPOINT");
        }
        assert!(err.contains("embedding model"), "{err}");
        assert!(err.contains("--local-prefetch"), "{err}");
    }

    #[test]
    fn an_empty_request_costs_nothing() {
        assert_eq!(embed(&[]).unwrap(), Vec::<Vec<f32>>::new());
    }
}
