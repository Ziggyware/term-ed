# Local ONNX inference for termed

`onnx_server.py` serves an OpenAI-compatible `/v1` API from a local ONNX
model using [ONNX Runtime](https://onnxruntime.ai). It is the backend for
termed's `local` provider; you can also run it by hand.

## Setup

```sh
pip install -r local_inference/requirements.txt
```

## Run by hand

```sh
python3 local_inference/onnx_server.py --model onnx-community/SmolLM2-135M-Instruct
# or a local model directory containing model.onnx + tokenizer.json:
python3 local_inference/onnx_server.py --model /path/to/model-dir --port 8080
```

Then point termed at it (Settings → AI agent configuration → **Built-in
provider: `local`**, or launch with `termed --local`). The default endpoint
is `http://127.0.0.1:8080/v1`.

## Choosing a model

Any Hugging Face repo with ONNX weights + `tokenizer.json` works. Set the
**Model** field to the repo id (or a local directory path) to use it.

## Tests

```sh
pip install -r local_inference/requirements-dev.txt   # adds `onnx`
python3 -m unittest discover -s local_inference/tests -v
```
