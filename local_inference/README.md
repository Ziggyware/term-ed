# Local ONNX inference for termed

`onnx_server.py` serves an OpenAI-compatible `/v1` API from a local ONNX
model using [ONNX Runtime](https://onnxruntime.ai). It is the backend for
termed's `local` provider and the automatic fallback when the remote
provider returns HTTP 503. termed embeds this exact file in the binary and
starts it for you when the local endpoint is on loopback; you can also run
it by hand.

## Setup

```sh
pip install -r local_inference/requirements.txt
```

If Hugging Face is unreachable, set `HF_ENDPOINT` to a mirror or pre-download
into the cache with `huggingface-cli`; the server also retries dropped
connections instead of failing the fallback. See
[docs/local-inference.md](../docs/local-inference.md#when-hugging-face-is-unreachable).

The first run downloads the default model
(`onnx-community/SmolLM2-135M-Instruct`, ~150 MB) from Hugging Face into
`~/.config/termed/onnx-models/`. After that it runs fully offline.

## Run by hand

```sh
python3 local_inference/onnx_server.py --model onnx-community/SmolLM2-135M-Instruct
# or a local model directory containing model.onnx + tokenizer.json:
python3 local_inference/onnx_server.py --model /path/to/model-dir --port 8080
```

Then point termed at it (Settings → AI agent configuration → **Built-in
provider: `local`**, or launch with `termed --local`). The default endpoint
is `http://127.0.0.1:8080/v1`; override with the **Local ONNX endpoint**
setting or `TERMED_LOCAL_ENDPOINT`.

## Choosing a model

Any Hugging Face repo with ONNX weights + `tokenizer.json` works, for
example `onnx-community/Qwen2.5-0.5B-Instruct` for better answers at the
cost of more RAM and slower CPU generation. Set the **Model** field to the
repo id (or a local directory path) to use it. Small on-device models do
not reliably call tools, so local mode answers in plain text; use the
remote provider for full agentic coding.

## Tests

```sh
pip install -r local_inference/requirements-dev.txt   # adds `onnx`
python3 -m unittest discover -s local_inference/tests -v
```

There are 24 tests: the API surface, streaming, cancellation mid-stream, the
loading/failed states, model-path validation, shutdown with and without a
request body, and Hub download retry (a dropped connection is retried, while a
persistent one explains the mirror and offline options). All of it runs against
a tiny deterministic ONNX model built inside the test — no network, no
download.
