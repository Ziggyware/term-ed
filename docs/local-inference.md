# Local ONNX inference

termed can run the built-in agent on a model **on your own machine**, using
[ONNX Runtime](https://onnxruntime.ai) and no network connection and no API
key. It is used in two ways:

- **Automatically when the remote provider fails.** If the provider (for
  example the NoKey Router) answers **HTTP 503** — or any other 5xx — or
  rejects its key (HTTP **401/403**), the current turn continues on the
  local model instead of ending with an authentication or failover error.
  The agent panel says so, and the model picker shows the local model while
  the fallback is active.
- **As the selected provider.** Choose **Use Local ONNX Inference** in **AI
  agent configuration**, set `"provider": "local"` in settings JSON, or start
  termed with `--local`. Every turn then runs on this machine.

The built-in AI chat can also download and run the local model by itself,
without changing the configured provider, when you ask it to use on-device
inference. It has a `local_inference` tool: `status` reports whether the
server is running, `download` fetches the model into the local cache (no
ONNX Runtime required), and `start` starts the embedded server and switches
this session to on-device inference. If it fails, the chat reports the fix
(install the dependencies, check `~/.config/termed/onnx-server.log`, or run
`termed --local-serve`).

If local inference also fails, the turn returns to the configured provider,
and if that fails too one message explains both. Local inference never
receives the router key, and no prompt leaves the machine.

## Requirements

- Python 3.9+ with ONNX Runtime:

  ```sh
  pip install -r local_inference/requirements.txt
  ```

  termed runs `python3` (or `python` on Windows, or `$TERMED_PYTHON`). A
  virtual environment works: set `TERMED_PYTHON=/path/to/venv/bin/python`.
  On systems that mark the interpreter as externally managed (Debian,
  Ubuntu 23.04+), use a virtual environment or
  `pipx install onnxruntime tokenizers` equivalents; termed only needs the
  modules importable by that interpreter.

- The default model, `onnx-community/SmolLM2-135M-Instruct` (~150 MB), is
  downloaded from Hugging Face on first use into
  `$XDG_CONFIG_HOME/termed/onnx-models/`. After that, inference is fully
  offline. Set **Model** to another ONNX repo id (for example
  `onnx-community/Qwen2.5-0.5B-Instruct`) or to a local directory containing
  `model.onnx` and `tokenizer.json` to use something else.

### When Hugging Face is unreachable

The server retries the file listing and each download with backoff (4 attempts
for the listing, 3 per file), because drops like Windows `WinError 10054`
("an existing connection was forcibly closed", typical of proxies, VPNs and
filtering middleboxes) are usually transient. Downloaded files are written as
`.part` and renamed only when complete, so a retry resumes cleanly and never
leaves a corrupt cache.

If the Hub is blocked outright, point at a mirror or pre-download:

```sh
# A reachable mirror (e.g. hf-mirror.com)
HF_ENDPOINT=https://hf-mirror.com termed --local-serve

# Or fetch into termed's cache ahead of time, so an outage never waits on it
termed --local-prefetch          # uses the embedded server; needs no onnxruntime

# The same thing with the Hugging Face client, if you prefer a progress bar
pip install huggingface_hub
huggingface-cli download onnx-community/SmolLM2-135M-Instruct \
  --include "*.onnx" "*.onnx.data" "tokenizer.json" "tokenizer_config.json" \
            "config.json" "generation_config.json" \
  --local-dir ~/.config/termed/onnx-models/onnx-community--SmolLM2-135M-Instruct
```

On Windows the cache path is
`%USERPROFILE%\.config\termed\onnx-models\onnx-community--SmolLM2-135M-Instruct`,
and `huggingface-cli` is `huggingface-cli.exe`. Any directory with
`model.onnx` and `tokenizer.json` also works directly: set **Model** to that
path and nothing is fetched at all. Running `termed --local-serve` once
downloads the model with visible progress, so a later 503 fallback starts in
about a second.

Model files are cached per repository; once present, no network is used at
all, and a cached model is verified without touching the Hub.

termed writes the embedded server to
`$XDG_CONFIG_HOME/termed/onnx_server.py`, starts it on the loopback address
of the configured endpoint, and waits for `/healthz` to report the model
loaded. The server keeps running after termed exits so the next launch
reuses it; stop it with `POST /admin/shutdown`, or `pkill -f onnx_server.py`.
Logs go to `$XDG_CONFIG_HOME/termed/onnx-server.log`.

## Settings

| Setting | Meaning |
| --- | --- |
| Built-in provider | `local` (alias `onnx`) runs every turn on this machine; `router`, `auto`, `kilo` and `custom` keep their remote/ACP meaning |
| Local ONNX endpoint | Local server base URL; empty uses `http://127.0.0.1:8080/v1`. Also `--local-endpoint` or `TERMED_LOCAL_ENDPOINT` |
| Model | ONNX repo id or local model directory; empty loads the default model |

Only a loopback `http://` endpoint (`127.0.0.1`, `localhost`, `::1`) is
started automatically. A remote or HTTPS endpoint must already be serving
the same API, and termed reports how to start it if not. The API base URL
field is ignored while the local provider is selected, so a stale URL cannot
route prompts back to the network.

`--local-serve` runs the server in the foreground (useful for trying models,
or on a machine where termed cannot start it):

```sh
termed --local-serve                          # 127.0.0.1:8080, default model
termed --local-serve --local-model onnx-community/Qwen2.5-0.5B-Instruct
termed --local-serve --local-port 9090
termed --local-prefetch                       # just download/verify the model
```

## What to expect

Small on-device models are much weaker than hosted ones, and most do not
call tools reliably. In local mode the agent is told to answer from the
conversation and to call at most one tool at a time; expect prose answers and
small edits rather than long autonomous coding runs. The output budget is
capped at 1,024 tokens per reply so a slow CPU model cannot hold the turn for
the whole task timeout. Tool calling, when the model manages it, uses the
same tools and approvals as the remote provider.

For serious agentic work, keep the router or another hosted provider as the
primary and let local inference cover outages.

## How the fallback works

`src/builtin_agent.rs` handles one user turn as a loop of model calls. When a
call fails with a 5xx status, the built-in agent:

1. Starts (or attaches to) the local server at the **Local ONNX endpoint**
   and records its model id.
2. Announces the switch in the agent panel and updates the model picker.
3. Resends the same conversation — system prompt, tool results so far, and
   the user request — to the local model.
4. If the local call fails, restores the previous provider and model and
   ends the turn with one message naming both failures.

A 429 from a free provider is still handled by model failover inside the
router, without touching local inference.

## Tests

```sh
pip install -r local_inference/requirements-dev.txt          # adds `onnx`
python3 -m unittest discover -s local_inference/tests -v     # 24 server tests
cargo test --test local_inference                            # 503 → local, end to end
```

The server tests build a tiny deterministic ONNX model in the test, so they
need no network and no model download. The Rust test uses the committed
fixture model in `tests/fixtures/tiny_onnx_model` and the 503 gateway in
`tests/fixtures/mock_503_gateway.py`, and skips itself when `python3` cannot
import `onnxruntime`, `tokenizers` and `numpy`.
