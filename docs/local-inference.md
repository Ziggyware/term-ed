# Local ONNX inference

termed can run the built-in agent on a model **on your own machine**, using ONNX Runtime, with no network connection and no API key. It is used in two ways:

- **Automatically when the remote provider fails.** The current turn continues on the local model instead of ending with an error.
- **As the selected provider.** Choose **Use Local ONNX Inference** in AI agent configuration, set `"provider": "local"` in settings JSON, or start termed with `--local`. Every turn then runs on this machine.

## Requirements

- Python 3.9+ with ONNX Runtime:

  ```sh
  pip install -r local_inference/requirements.txt
  ```

## Model selection

The default model is `onnx-community/SmolLM2-135M-Instruct` (~150 MB), downloaded from Hugging Face on first use. After that, inference is fully offline. Set **Model** to another ONNX repo id (for example `onnx-community/Qwen2.5-0.5B-Instruct`) or to a local directory containing `model.onnx` and `tokenizer.json` to use something else.

## Settings

| Setting | Meaning |
|---|---|
| Built-in provider | `local` runs every turn on this machine (the `onnx` alias is normalized to `local` on load and save) |
| Local ONNX endpoint | Local server base URL; empty uses `http://127.0.0.1:8080/v1`. Also `--local-endpoint` or `TERMED_LOCAL_ENDPOINT` |
| Model | ONNX repo id or local model directory; empty loads the default model |

## CLI commands

`--local-serve` runs the server in the foreground:

```sh
termed --local-serve                          # 127.0.0.1:8080, default model
termed --local-serve --local-model onnx-community/Qwen2.5-0.5B-Instruct
termed --local-serve --local-port 9090
termed --local-prefetch                       # just download/verify the model
```
