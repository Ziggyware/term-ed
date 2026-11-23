# Code Intelligence, Semantic Search & Unattended Review

Three features sharing one principle: cheap, checkable local analysis — never change code without user approval.

## Context Menu (`Ctrl+Shift+Space`)

Opens for the symbol under caret; type to filter, `Esc` to close.

| Action | Key | Behavior |
|---|---|---|
| Go to Definition | `F12` | Jump to declaration in open buffer |
| Find All References | `Shift+F12` | Whole-word search across project |
| Inspect Symbol | `Ctrl+F2` | Declaration count, use count, likely problems |
| Rename Symbol | `F2` | Rename every occurrence with preview |
| Copy File:Line | — | `path:line` name to clipboard |
| Extract Enclosing Block | — | Copy function to new buffer |
| Sort / Dedup Lines | — | Selection or whole buffer |

## Semantic Search (`Ctrl+Alt+F`)

Search: Find Related Code — indexes project with on-device ONNX encoder, ranks against selection/symbol. Finds code by what it does, not just name.

## Unattended Review

Agent: Review the Project for Obvious Bugs — analyzes project, lists findings. Nothing written without approval unless user opts into high-confidence fixes.

| Finding | Confidence | Rationale |
|---|---|---|
| Unbalanced brackets | High | File cannot parse |
| Block repeated verbatim | Medium | Almost always copy-paste error |
| Empty catch/except | Medium | Silently hides failures |
| TODO/FIXME/HACK | Low | May be intentional |
| Comment admitting bug | Low | Only human knows if still true |
| Declaration unused in file | Low | May be used elsewhere |

## Tests

cargo test --lib code_intel        # words, definitions, rename refusals
cargo test --lib embeddings        # chunking, cosine, ranking, failure text
cargo test --lib autofix           # each finding, approval rules
cargo test --lib a2a               # task lifecycle, capability negotiation
python3 -m unittest discover -s local_inference/tests   # 29 server tests
