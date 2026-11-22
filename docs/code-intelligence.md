# Code intelligence, semantic search and unattended review

Three related features that share one idea: do the cheap, checkable analysis locally, and never change code without the user seeing it first.

## Context menu: definitions, references, inspection, refactors

`Ctrl+Shift+Space` opens a menu for the symbol under the caret. Typing filters it and Esc closes it.

| Action | Key | What it does |
|---|---|---|
| Go to Definition | `F12` | Jump to the declaration in the open buffer |
| Find All References | `Shift+F12` | Whole-word search across the project |
| Inspect Symbol | `Ctrl+F2` | Declaration count, use count, likely problems |
| Rename Symbol | `F2` | Rename every occurrence, with a preview of the count |
| Copy File:Line | — | `path:line name` on the clipboard |
| Extract Enclosing Block | — | Copy the function into a new buffer |
| Sort / Remove Duplicate Lines | — | Operate on the selection, or the whole buffer |

This is **not** a language server. There is no install step, no background process and no per-language grammar: the analysis reads the buffer with defined rules (`src/code_intel.rs`), so it works for every file type the editor can highlight, including formats no language server exists for.

That trade-off is deliberate and visible:

- **Rename refuses rather than guessing.** It will not rename to a reserved word, to something already used where the symbol is declared, or when the name appears zero times. `cat` never renames `category`.
- **Inspect admits uncertainty.** Two declarations of the same name are reported as "declared twice", not resolved to one.
- **Go to Definition is file-scoped.** If the declaration is not in the open buffer it falls back to Find All References instead of silently jumping somewhere wrong.

## Semantic search with ONNX embeddings

`Ctrl+Alt+F` (or **Search: Find Related Code**) indexes the project with a small on-device encoder and ranks it against the selection or the symbol under the caret. It finds code that *does* the thing when you cannot remember what it is *called*.

Nothing leaves the machine: vectors come from the same local server that serves chat, over a loopback connection.

### Supporting files

Embeddings are written to the same API as chat:

```sh
POST /v1/embeddings
{"model": "Xenova/all-MiniLM-L6-v2", "input": ["fn main() {}", "..."]}
```

The response is OpenAI-shaped: `{"data": [{"index": 0, "embedding": [...]}]}`.
Vectors are mean-pooled over the attention mask and L2-normalised, so a cosine score is directly comparable across inputs and batch sizes.

### The embedding model is separate from the chat model

Start the server with both, or only the one you need:

```sh
termed --local-serve                                   # chat only
python3 onnx_server.py --embedding-model Xenova/all-MiniLM-L6-v2
```

The encoder is optional by design: a missing embedding model never breaks chat. `/healthz` reports `"embeddings": "failed"` alongside `"status": "ok"`, and `/v1/embeddings` answers 503 with the flag that enables it.

Fetch it ahead of time — it is small (~23 MB quantised), so this is quick:

```sh
python3 onnx_server.py --prewarm --prewarm-embeddings
```

Change the model under **Settings → Backends → Embeddings**, or with `TERMED_EMBEDDING_MODEL`. A directory containing `model.onnx` and `tokenizer.json` works too.

### When it cannot run

Without an encoder the reply says exactly what to do instead of returning an empty list:

> Semantic search needs a local embedding model: … Run `termed --local-prefetch --local-embedding-model` to download one, or set an endpoint with `TERMED_EMBEDDING_ENDPOINT`.

## Unattended review

**Agent: Review the Project for Obvious Bugs** analyses the project and lists what it found. The rule the whole feature is built around:

> Nothing is written without approval unless the user has explicitly opted in to high-confidence fixes.

`src/autofix.rs` is pure and offline — it reads files and produces proposals. It never writes, never runs a command and never touches the network, which is what makes it safe to run on a timer.

| Finding | Confidence | Why that level |
|---|---|---|
| Unbalanced brackets | High | The file cannot parse |
| A block repeated verbatim | Medium | Almost always a copy-paste error |
| An empty `catch`/`except` | Medium | Silently hides failures |
| `TODO` / `FIXME` / `HACK` marker | Low | May be intentional and pending |
| A comment admitting a bug | Low | Only a human knows if it is still true |
| A declaration never used in its file | Low | May be used from elsewhere |

Confidence is not a score, it is a permission level. Only **High** can ever be applied unattended, and even then the default policy forbids it:

```rust
Policy {
    enabled: false,              // analysis off until you ask for it
    apply_high_confidence: false, // every change needs a person
    ..Policy::default()
}
```

**Settings → Backends → Unattended fixes** toggles analysis and states, in the row itself, whether changes need approval. The panel never claims to have applied something it did not.

Findings are capped (`max_proposals`, default 100) and bounded by file size (`max_file_bytes`, default 512 KB), so a large tree cannot flood the panel.

## Backends panel

**Settings → Backends** (`Ctrl+,` then the Backends row) shows live status and lets you change it:

- **Jev decisions** — whether judgments come from the TypeSafe cloud or the offline local heuristic.
- **MCP tools** — how many are exposed, and whether the bridge is connected.
- **Embeddings** — the encoder in use for semantic search.
- **Unattended fixes** — current policy, in plain words.

## Tests

```sh
cargo test --lib code_intel        # words, definitions, rename refusals
cargo test --lib embeddings        # chunking, cosine, ranking, failure text
cargo test --lib autofix           # each finding, and the approval rules
cargo test --lib a2a               # task lifecycle, capability negotiation
python3 -m unittest discover -s local_inference/tests   # 29 server tests
```

The Rust tests need no model, no network and no ONNX Runtime: the strategy is pure functions in, values out.