#!/usr/bin/env python3
"""termed local ONNX inference server.

Serves an OpenAI-compatible ``/v1`` API from a local ONNX model using ONNX
Runtime, so termed's built-in agent can run inference on this machine with
no network and no API key. This is the fallback used when the remote
provider returns HTTP 503, and the backend for the ``local`` provider.

Usage:
    python3 onnx_server.py --model onnx-community/SmolLM2-135M-Instruct
    python3 onnx_server.py --model /path/to/model-dir   # local dir with model.onnx + tokenizer.json

Endpoints:
    GET  /healthz                  {"status": "ok"|"loading"|"failed", ...}
    GET  /v1/models                OpenAI model list
    POST /v1/chat/completions      OpenAI chat completions (streaming SSE supported)
    POST /admin/shutdown           stop the server (loopback convenience for termed)

The model is loaded in a background thread: /healthz reports "loading"
until the model is ready, then "ok" (or "failed" with the error). Chat
requests before the model is ready get HTTP 503, matching the remote
provider's failure mode so clients handle both the same way.

Tool calling is not supported by small on-device models: a ``tools`` field
in the request is accepted and ignored, and the reply is plain text.

Requires: onnxruntime, tokenizers, numpy (see requirements.txt).
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

__version__ = "1.0.0"
DEFAULT_MODEL = "onnx-community/SmolLM2-135M-Instruct"
MAX_BODY_BYTES = 2 * 1024 * 1024
MAX_OUTPUT_TOKENS = 8192
USER_AGENT = f"termed-onnx-server/{__version__}"
# Hub base URL. Override with HF_ENDPOINT (e.g. https://hf-mirror.com) when
# huggingface.co is blocked or resets connections on a corporate network.
HUB = os.environ.get("HF_ENDPOINT", "https://huggingface.co").rstrip("/")
# (connect+read) timeouts per attempt; network resets are common enough that a
# single attempt is not good enough to decide a model cannot be fetched.
LIST_TIMEOUT = 30
DOWNLOAD_TIMEOUT = 120


class ClientGone(Exception):
    """The client closed the connection mid-stream (for example a cancelled turn)."""


class HubError(RuntimeError):
    """The model could not be fetched from the Hub after several attempts."""


def log(msg: str) -> None:
    print(f"[onnx-server] {msg}", file=sys.stderr, flush=True)


def default_cache_dir() -> str:
    xdg = os.environ.get("XDG_CONFIG_HOME", "")
    base = xdg if os.path.isabs(xdg) else os.path.expanduser("~/.config")
    return os.path.join(base, "termed", "onnx-models")


# --------------------------------------------------------------------------
# Model resolution / download
# --------------------------------------------------------------------------

def _open(req: urllib.request.Request, *, timeout: int, label: str,
          attempts: int = 4):
    """urlopen with exponential backoff.

    A dropped TLS connection (WinError 10054 / ECONNRESET, proxy, VPN, or a
    briefly unavailable Hub) used to abort the whole local fallback. These
    are transient: retry before giving up. A definitive 4xx answer (a typo'd
    repo id, a repo that does not exist) is NOT transient: retrying it
    wastes minutes, so it raises immediately.
    """
    last: Exception | None = None
    for attempt in range(attempts):
        try:
            return urllib.request.urlopen(req, timeout=timeout)
        except urllib.error.HTTPError as exc:
            if exc.code == 429 or exc.code >= 500:
                # Rate-limited or a broken Hub: try again, like a drop.
                last = exc
            else:
                # A definitive answer — a bad model id or a deleted repo.
                raise HubError(
                    f"{label} failed: the Hub answered HTTP {exc.code}. "
                    f"Check the model id (it must be a Hugging Face repo "
                    f"id, e.g. onnx-community/SmolLM2-135M-Instruct), or pass "
                    f"--model with a local directory (see docs/local-inference.md)."
                ) from exc
            if attempt + 1 == attempts:
                break
            delay = 2 ** attempt
            log(f"{label} failed ({exc}); retrying in {delay}s "
                f"[{attempt + 1}/{attempts}]")
            time.sleep(delay)
        except (urllib.error.URLError, OSError, ValueError) as exc:
            last = exc
            if attempt + 1 == attempts:
                break
            delay = 2 ** attempt
            log(f"{label} failed ({exc}); retrying in {delay}s "
                f"[{attempt + 1}/{attempts}]")
            time.sleep(delay)
    assert last is not None
    if isinstance(last, ValueError):
        raise last
    raise HubError(
        f"{label} failed after {attempts} attempts ({last}). "
        f"Set HF_ENDPOINT to a reachable mirror, pass --model with a local "
        f"directory, or pre-download with huggingface-cli "
        f"(see docs/local-inference.md)."
    ) from last


def _download(url: str, dest: str) -> None:
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    tmp = dest + ".part"
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    log(f"downloading {url}")
    with _open(req, timeout=DOWNLOAD_TIMEOUT, label=f"downloading {url}",
               attempts=3) as resp, open(tmp, "wb") as out:
        while True:
            chunk = resp.read(1024 * 256)
            if not chunk:
                break
            out.write(chunk)
    os.replace(tmp, dest)


# Small encoder for /v1/embeddings. ~23 MB quantised, so it is cheap to fetch
# and fast on a CPU; sentence-transformers/all-MiniLM-L6-v2 is the same model.
DEFAULT_EMBEDDING_MODEL = "Xenova/all-MiniLM-L6-v2"


def _hf_wanted_files(repo: str) -> list[str]:
    api = f"{HUB}/api/models/{repo}"
    req = urllib.request.Request(api, headers={"User-Agent": USER_AGENT})
    with _open(req, timeout=LIST_TIMEOUT,
               label=f"listing files for {repo}") as resp:
        data = json.loads(resp.read().decode("utf-8"))
    names = [s.get("rfilename", "") for s in data.get("siblings", [])]
    keep = {
        "tokenizer.json",
        "tokenizer_config.json",
        "config.json",
        "generation_config.json",
        "special_tokens_map.json",
        "added_tokens.json",
        "vocab.json",
        "merges.txt",
        "vocab.txt",
    }
    wanted = [
        n
        for n in names
        if n.endswith((".onnx", ".onnx.data")) or os.path.basename(n) in keep
    ]
    return sorted(set(wanted))


def path_like(model: str) -> bool:
    """Whether `model` names a filesystem path rather than a Hub repo id."""
    return (
        model.startswith(("~", "/", "./", "../", ".." + os.sep))
        or (len(model) > 1 and model[1] == ":")  # Windows drive
    )


def resolve_model_dir(model: str, cache_dir: str) -> str:
    """Return a local directory containing the ONNX model + tokenizer."""
    if os.path.isdir(model):
        return model
    if os.path.isfile(model):
        # A direct path to a .onnx file; tokenizer must sit alongside it.
        return os.path.dirname(os.path.abspath(model))
    if path_like(model):
        # A typo must not be mistaken for a Hugging Face repo id.
        raise RuntimeError(
            f"model directory {model!r} does not exist; it needs model.onnx "
            "and tokenizer.json"
        )
    parts = model.split("/")
    if (
        len(parts) != 2
        or not all(p.strip() for p in parts)
        or any(c.isspace() for c in model)
    ):
        # Validate before any network access: a bare name or `a/b/c` is a
        # typo, not a repository.
        raise RuntimeError(
            f"{model!r} is not a Hugging Face repo id (owner/name) or a local directory"
        )
    cache = os.path.join(cache_dir, model.replace("/", "--"))
    if find_onnx_file(cache) and os.path.isfile(os.path.join(cache, "tokenizer.json")):
        return cache
    log(f"fetching model {model} into {cache}")
    wanted = _hf_wanted_files(model)
    if not any(n.endswith(".onnx") for n in wanted):
        raise RuntimeError(f"model {model!r} has no .onnx files")
    if "tokenizer.json" not in [os.path.basename(n) for n in wanted]:
        raise RuntimeError(f"model {model!r} has no tokenizer.json")
    for name in wanted:
        dest = os.path.join(cache, name)
        if not os.path.isfile(dest):
            _download(f"{HUB}/{model}/resolve/main/{name}", dest)
    return cache


def prewarm(model: str, cache_dir: str) -> str:
    """Download (or verify) a model without loading ONNX Runtime.

    Lets a user fetch the model before an outage, on a machine that may not
    have onnxruntime installed yet, so a later 503 fallback starts instantly.
    """
    model_dir = resolve_model_dir(model, cache_dir)
    if not find_onnx_file(model_dir):
        raise RuntimeError(f"no .onnx file in {model_dir}")
    if not os.path.isfile(os.path.join(model_dir, "tokenizer.json")):
        raise RuntimeError(f"no tokenizer.json in {model_dir}")
    return model_dir


def find_onnx_file(model_dir: str) -> str | None:
    for cand in (
        "model.onnx",
        os.path.join("onnx", "model.onnx"),
        "model_quantized.onnx",
        os.path.join("onnx", "model_quantized.onnx"),
    ):
        path = os.path.join(model_dir, cand)
        if os.path.isfile(path):
            return path
    found: list[str] = []
    for root, _dirs, files in os.walk(model_dir):
        for name in files:
            if name.endswith(".onnx"):
                found.append(os.path.join(root, name))
    return sorted(found)[0] if found else None


# --------------------------------------------------------------------------
# Chat templates (string rendering for the common formats)
# --------------------------------------------------------------------------

def _content_text(content) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "".join(
            part.get("text", "") for part in content if isinstance(part, dict)
        )
    return ""


def render_chat(messages: list[dict], template: str | None) -> str:
    if template and "<|im_start|>" in template:
        parts = []
        for m in messages:
            parts.append(
                f"<|im_start|>{m.get('role', 'user')}\n"
                f"{_content_text(m.get('content', ''))}<|im_end|>\n"
            )
        parts.append("<|im_start|>assistant\n")
        return "".join(parts)
    if template and "<|start_header_id|>" in template:
        parts = ["<|begin_of_text|>"]
        for m in messages:
            parts.append(
                f"<|start_header_id|>{m.get('role', 'user')}<|end_header_id|>\n\n"
                f"{_content_text(m.get('content', ''))}<|eot_id|>"
            )
        parts.append("<|start_header_id|>assistant<|end_header_id|>\n\n")
        return "".join(parts)
    if template and "[INST]" in template:
        parts = []
        for m in messages:
            role = m.get("role", "user")
            text = _content_text(m.get("content", ""))
            if role == "assistant":
                parts.append(f"{text} [/INST]")
            elif role == "system":
                parts.append(f"[INST] <<SYS>>\n{text}\n<</SYS>>\n\n")
            else:
                parts.append(f"[INST] {text} [/INST]")
        return "".join(parts)
    lines = [
        f"{m.get('role', 'user')}: {_content_text(m.get('content', ''))}"
        for m in messages
    ]
    lines.append("assistant:")
    return "\n".join(lines)


# --------------------------------------------------------------------------
# ONNX engine
# --------------------------------------------------------------------------

class OnnxEngine:
    """Autoregressive generation over an ONNX Runtime session."""

    def __init__(self, model_dir: str):
        import numpy as np
        import onnxruntime as ort
        from tokenizers import Tokenizer

        self.np = np
        onnx_path = find_onnx_file(model_dir)
        if not onnx_path:
            raise RuntimeError(f"no .onnx file found in {model_dir}")
        tok_path = os.path.join(model_dir, "tokenizer.json")
        if not os.path.isfile(tok_path):
            raise RuntimeError(f"no tokenizer.json found in {model_dir}")
        log(f"loading ONNX model {onnx_path}")
        opts = ort.SessionOptions()
        opts.intra_op_num_threads = os.cpu_count() or 1
        self.session = ort.InferenceSession(
            onnx_path, sess_options=opts, providers=["CPUExecutionProvider"]
        )
        self.tokenizer = Tokenizer.from_file(tok_path)
        self.chat_template = self._chat_template(model_dir)
        self.eos_ids = self._eos_ids(model_dir)
        inputs = {i.name: i for i in self.session.get_inputs()}
        self.input_ids_name = "input_ids" if "input_ids" in inputs else next(iter(inputs))
        self.attention_name = "attention_mask" if "attention_mask" in inputs else None
        self.position_name = "position_ids" if "position_ids" in inputs else None
        self.cache_position_name = (
            "cache_position" if "cache_position" in inputs else None
        )
        self.past_names = [n for n in inputs if n.startswith("past_key_values")]
        self.past_shapes = {
            n: self._past_shape(inputs[n], model_dir) for n in self.past_names
        }
        outputs = [o.name for o in self.session.get_outputs()]
        self.logits_name = outputs[0]
        self.past_output_names = [n for n in self.past_names if n in outputs]
        handled = {
            self.input_ids_name,
            self.attention_name,
            self.position_name,
            self.cache_position_name,
            *self.past_names,
        }
        unknown = [n for n in inputs if n not in handled]
        if unknown:
            raise RuntimeError(
                f"model needs unsupported ONNX inputs {unknown}; "
                "this server supports input_ids, attention_mask, position_ids, "
                "cache_position and past_key_values"
            )
        self.rng = np.random.default_rng()
        log(f"model ready: {onnx_path} (past_key_values: {len(self.past_names)})")

    def _chat_template(self, model_dir: str) -> str | None:
        path = os.path.join(model_dir, "tokenizer_config.json")
        try:
            with open(path, encoding="utf-8") as fh:
                data = json.load(fh)
            template = data.get("chat_template")
            return template if isinstance(template, str) else None
        except (OSError, ValueError):
            return None

    def _eos_ids(self, model_dir: str) -> list[int]:
        ids: list[int] = []
        for name in ("config.json", "generation_config.json"):
            try:
                with open(os.path.join(model_dir, name), encoding="utf-8") as fh:
                    data = json.load(fh)
                value = data.get("eos_token_id")
                if isinstance(value, int):
                    ids.append(value)
                elif isinstance(value, list):
                    ids.extend(v for v in value if isinstance(v, int))
            except (OSError, ValueError):
                continue
        if not ids:
            eos = getattr(self.tokenizer, "eos_token_id", None)
            if isinstance(eos, int):
                ids.append(eos)
        return sorted(set(ids))

    def _past_shape(self, node_arg, model_dir: str) -> tuple[int, int, int, int]:
        shape = list(node_arg.shape or [])
        heads = head_dim = None
        try:
            with open(os.path.join(model_dir, "config.json"), encoding="utf-8") as fh:
                cfg = json.load(fh)
            heads = cfg.get("num_attention_heads") or cfg.get("num_key_value_heads")
            hidden = cfg.get("hidden_size")
            if heads and hidden and hidden % heads == 0:
                head_dim = hidden // heads
        except (OSError, ValueError):
            pass

        def dim(index: int, fallback: int | None) -> int:
            if index < len(shape) and isinstance(shape[index], int):
                return shape[index]
            if fallback is not None:
                return fallback
            raise RuntimeError(
                f"cannot determine shape of ONNX input {node_arg.name}; "
                "provide config.json with num_attention_heads/hidden_size"
            )

        return (dim(0, 1), dim(1, heads), dim(2, 0), dim(3, head_dim))

    def encode(self, text: str) -> list[int]:
        return self.tokenizer.encode(text).ids

    def decode(self, ids: list[int]) -> str:
        return self.tokenizer.decode(ids, skip_special_tokens=True)

    def _pick(self, logits, temperature: float, top_p: float) -> int:
        np = self.np
        row = logits[0, -1, :].astype(np.float64)
        if temperature <= 0:
            return int(np.argmax(row))
        row = row / max(temperature, 1e-6)
        row = row - row.max()
        probs = np.exp(row)
        probs /= probs.sum()
        if 0 < top_p < 1:
            order = np.argsort(probs)[::-1]
            cumulative = np.cumsum(probs[order])
            keep = cumulative <= top_p
            keep[0] = True
            mask = np.zeros_like(probs, dtype=bool)
            mask[order[keep]] = True
            probs = np.where(mask, probs, 0.0)
            probs /= probs.sum()
        return int(self.rng.choice(len(probs), p=probs))

    def generate_tokens(
        self,
        prompt_ids: list[int],
        max_tokens: int,
        temperature: float,
        top_p: float,
    ):
        """Yield generated token ids one at a time."""
        np = self.np
        ids = list(prompt_ids)
        past: dict[str, object] = {}
        past_len = 0
        use_cache = bool(self.past_names)
        for step in range(max_tokens):
            if use_cache and step > 0:
                step_ids = [ids[-1]]
            else:
                step_ids = ids
            seq = len(step_ids)
            feeds: dict[str, object] = {
                self.input_ids_name: np.array([step_ids], dtype=np.int64)
            }
            total = past_len + seq
            if self.attention_name:
                feeds[self.attention_name] = np.ones((1, total), dtype=np.int64)
            if self.position_name:
                feeds[self.position_name] = np.arange(
                    past_len, total, dtype=np.int64
                ).reshape(1, -1)
            if self.cache_position_name:
                feeds[self.cache_position_name] = np.arange(
                    past_len, total, dtype=np.int64
                )
            if use_cache:
                for name in self.past_names:
                    if name in past:
                        feeds[name] = past[name]
                    else:
                        feeds[name] = np.zeros(
                            (self.past_shapes[name][0], self.past_shapes[name][1], 0,
                             self.past_shapes[name][3]),
                            dtype=np.float32,
                        )
            outputs = self.session.run(
                [self.logits_name, *self.past_output_names], feeds
            )
            logits = outputs[0]
            if use_cache:
                past = {
                    name: outputs[1 + i]
                    for i, name in enumerate(self.past_output_names)
                }
                past_len = total
            token = self._pick(logits, temperature, top_p)
            if token in self.eos_ids:
                return
            ids.append(token)
            yield token


# --------------------------------------------------------------------------
# HTTP server
# --------------------------------------------------------------------------

class EmbeddingEngine:
    """A separate, much smaller ONNX model for `POST /embeddings`.

    Encoder models (MiniLM and friends) take `input_ids`/`attention_mask` and
    return a hidden state; there is no KV cache and no chat template, so this
    shares nothing with `OnnxEngine` except the ONNX Runtime call.
    """

    def __init__(self, model_dir: str):
        import numpy as np
        import onnxruntime as ort
        from tokenizers import Tokenizer

        self.np = np
        onnx_path = find_onnx_file(model_dir)
        if not onnx_path:
            raise RuntimeError(f"no .onnx file found in {model_dir}")
        tok_path = os.path.join(model_dir, "tokenizer.json")
        if not os.path.isfile(tok_path):
            raise RuntimeError(f"no tokenizer.json found in {model_dir}")
        opts = ort.SessionOptions()
        opts.intra_op_num_threads = os.cpu_count() or 1
        self.session = ort.InferenceSession(
            onnx_path, sess_options=opts, providers=["CPUExecutionProvider"]
        )
        self.tokenizer = Tokenizer.from_file(tok_path)
        if self.tokenizer.padding is None:
            # Batch inputs must be rectangular; pad to the longest in the batch.
            self.tokenizer.enable_padding()
        self.tokenizer.enable_truncation(max_length=512)
        names = {i.name for i in self.session.get_inputs()}
        self.input_name = "input_ids" if "input_ids" in names else "input_ids"
        self.mask_name = "attention_mask" if "attention_mask" in names else None
        self.type_ids_name = "token_type_ids" if "token_type_ids" in names else None
        self.output_name = self.session.get_outputs()[0].name
        log(f"embedding model ready: {onnx_path}")

    def embed(self, texts: list[str]) -> list[list[float]]:
        """Mean-pool the token vectors, then L2-normalise."""
        np = self.np
        encodings = self.tokenizer.encode_batch(texts)
        ids = np.array([e.ids for e in encodings], dtype=np.int64)
        feeds = {self.input_name: ids}
        if self.mask_name:
            feeds[self.mask_name] = np.array(
                [e.attention_mask for e in encodings], dtype=np.int64
            )
        if self.type_ids_name:
            feeds[self.type_ids_name] = np.array(
                [e.type_ids for e in encodings], dtype=np.int64
            )
        hidden = self.session.run([self.output_name], feeds)[0]
        mask = np.array([e.attention_mask for e in encodings], dtype=np.float32)
        mask = mask[..., None]                       # (batch, tokens, 1)
        summed = (hidden * mask).sum(axis=1)
        counts = np.clip(mask.sum(axis=1), 1e-9, None)
        pooled = summed / counts
        norms = np.linalg.norm(pooled, axis=1, keepdims=True)
        pooled = pooled / np.clip(norms, 1e-12, None)
        return pooled.astype(np.float32).tolist()


class State:
    def __init__(self, model_id: str, embedding_id: str | None = None):
        self.model_id = model_id
        self.engine: OnnxEngine | None = None
        self.error: str | None = None
        self.lock = threading.Lock()
        self.gen_lock = threading.Lock()
        # Embeddings are optional and load independently: a chat model that
        # works must not be taken down by a missing embedding model.
        self.embedding_id = embedding_id
        self.embeddings: EmbeddingEngine | None = None
        self.embedding_error: str | None = None
        self.embedding_loading = embedding_id is not None


STATE: State | None = None
MAX_TOKENS = MAX_OUTPUT_TOKENS


def error_body(message: str, code: str = "invalid_request_error") -> dict:
    return {"error": {"message": message, "type": code, "code": code}}


class Handler(BaseHTTPRequestHandler):
    server_version = f"termed-onnx/{__version__}"

    def _send_json(self, status: int, payload: dict) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _read_json(self):
        length = int(self.headers.get("Content-Length") or 0)
        if length <= 0 or length > MAX_BODY_BYTES:
            return None, (400 if length <= 0 else 413)
        raw = self.rfile.read(length)
        try:
            return json.loads(raw.decode("utf-8")), None
        except (ValueError, UnicodeDecodeError):
            return None, 400

    def do_GET(self) -> None:  # noqa: N802 (stdlib naming)
        path = self.path.split("?", 1)[0].rstrip("/") or "/"
        if path in ("/healthz", "/v1/healthz"):
            state = STATE
            assert state is not None
            if state.engine is not None:
                body = {
                    "status": "ok",
                    "model": state.model_id,
                    "version": __version__,
                }
                # Embeddings are reported separately: chat works even when the
                # encoder is missing, so `status` stays "ok".
                if state.embedding_id:
                    body["embeddings"] = (
                        "ok" if state.embeddings is not None
                        else ("failed" if state.embedding_error else "loading")
                    )
                    body["embedding_model"] = state.embedding_id
                self._send_json(200, body)
            elif state.error:
                self._send_json(503, {
                    "status": "failed",
                    "model": state.model_id,
                    "version": __version__,
                    "error": state.error,
                })
            else:
                self._send_json(503, {
                    "status": "loading",
                    "model": state.model_id,
                    "version": __version__,
                })
            return
        if path == "/v1/models":
            state = STATE
            assert state is not None
            self._send_json(200, {
                "object": "list",
                "data": [{
                    "id": state.model_id,
                    "object": "model",
                    "created": 0,
                    "owned_by": "termed-local-onnx",
                }],
            })
            return
        self._send_json(404, error_body(f"unknown route: {path}", "not_found"))

    def _handle_embeddings(self) -> None:
        """`POST /v1/embeddings` (OpenAI-compatible) for semantic search."""
        payload, err = self._read_json()
        if payload is None:
            self._send_json(err or 400, error_body("invalid JSON body"))
            return
        state = STATE
        assert state is not None
        if state.embedding_id is None:
            self._send_json(503, error_body(
                "no embedding model configured; start the server with --embedding-model",
                "embeddings_unavailable"))
            return
        if state.embeddings is None:
            if state.embedding_error:
                self._send_json(503, error_body(
                    f"embedding model failed to load: {state.embedding_error}",
                    "embeddings_unavailable"))
            else:
                self._send_json(503, error_body(
                    "embedding model is still loading", "model_loading"))
            return
        raw = payload.get("input")
        if isinstance(raw, str):
            texts = [raw]
        elif isinstance(raw, list) and all(isinstance(t, str) for t in raw):
            texts = raw
        else:
            self._send_json(400, error_body("input must be a string or list of strings"))
            return
        if not texts:
            self._send_json(400, error_body("input must not be empty"))
            return
        if len(texts) > 256:
            self._send_json(400, error_body("at most 256 inputs per request"))
            return
        try:
            vectors = state.embeddings.embed(texts)
        except Exception as exc:  # noqa: BLE001 - surfaced to the client
            self._send_json(500, error_body(f"embedding failed: {exc}", "embedding_failed"))
            return
        self._send_json(200, {
            "object": "list",
            "model": state.embedding_id,
            "data": [
                {"object": "embedding", "index": i, "embedding": v}
                for i, v in enumerate(vectors)
            ],
            "usage": {"prompt_tokens": sum(len(t.split()) for t in texts), "total_tokens": 0},
        })

    def do_POST(self) -> None:  # noqa: N802 (stdlib naming)
        path = self.path.split("?", 1)[0].rstrip("/") or "/"
        if path == "/admin/shutdown":
            self._send_json(200, {"status": "shutting_down"})
            threading.Thread(target=self.server.shutdown, daemon=True).start()
            return
        if path == "/v1/embeddings":
            self._handle_embeddings()
            return
        if path != "/v1/chat/completions":
            self._send_json(404, error_body(f"unknown route: {path}", "not_found"))
            return
        payload, err = self._read_json()
        if payload is None:
            self._send_json(err or 400, error_body("invalid JSON body"))
            return
        state = STATE
        assert state is not None
        if state.engine is None:
            if state.error:
                self._send_json(503, error_body(
                    f"local model failed to load: {state.error}", "model_unavailable"))
            else:
                self._send_json(503, error_body(
                    "local model is still loading", "model_loading"))
            return
        messages = payload.get("messages")
        if not isinstance(messages, list) or not messages or not all(
            isinstance(m, dict) and "role" in m for m in messages
        ):
            self._send_json(400, error_body("messages must be a non-empty list"))
            return
        max_tokens = payload.get("max_tokens", payload.get("max_completion_tokens", 512))
        try:
            max_tokens = int(max_tokens)
        except (TypeError, ValueError):
            self._send_json(400, error_body("max_tokens must be an integer"))
            return
        max_tokens = max(1, min(max_tokens, MAX_TOKENS))
        try:
            temperature = float(payload.get("temperature", 0.2))
        except (TypeError, ValueError):
            self._send_json(400, error_body("temperature must be a number"))
            return
        try:
            top_p = float(payload.get("top_p", 0.9))
        except (TypeError, ValueError):
            self._send_json(400, error_body("top_p must be a number"))
            return
        stop = payload.get("stop")
        stop_strings = [stop] if isinstance(stop, str) else (
            [s for s in stop if isinstance(s, str)] if isinstance(stop, list) else []
        )
        streaming = bool(payload.get("stream"))
        prompt = render_chat(messages, state.engine.chat_template)
        prompt_ids = state.engine.encode(prompt)
        completion_id = f"chatcmpl-{int(time.time() * 1000)}"
        created = int(time.time())
        with state.gen_lock:
            if streaming:
                self._stream(state, completion_id, created, prompt_ids,
                             max_tokens, temperature, top_p, stop_strings)
            else:
                self._complete(state, completion_id, created, prompt_ids,
                               max_tokens, temperature, top_p, stop_strings)

    def _generate_text(self, state, prompt_ids, max_tokens, temperature, top_p,
                       stop_strings, on_token=None):
        engine = state.engine
        assert engine is not None
        generated: list[int] = []
        text = ""
        finished = "length"
        for token in engine.generate_tokens(
            prompt_ids, max_tokens, temperature, top_p
        ):
            generated.append(token)
            if on_token is not None:
                # Decode only the completion: the prompt must never be
                # echoed back, and re-decoding it each token is wasteful.
                new_text = engine.decode(generated)
                delta = new_text[len(text):] if new_text.startswith(text) else new_text
                text = new_text
                if delta:
                    on_token(delta)
            if len(generated) >= max_tokens:
                break
        else:
            finished = "stop"
        if on_token is None:
            text = engine.decode(generated)
        for stop in stop_strings:
            if stop and stop in text:
                text = text.split(stop, 1)[0]
                finished = "stop"
                break
        return text, finished, len(generated)

    def _complete(self, state, completion_id, created, prompt_ids,
                  max_tokens, temperature, top_p, stop_strings) -> None:
        text, finish, n_tokens = self._generate_text(
            state, prompt_ids, max_tokens, temperature, top_p, stop_strings
        )
        self._send_json(200, {
            "id": completion_id,
            "object": "chat.completion",
            "created": created,
            "model": state.model_id,
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": finish,
            }],
            "usage": {
                "prompt_tokens": len(prompt_ids),
                "completion_tokens": n_tokens,
                "total_tokens": len(prompt_ids) + n_tokens,
            },
        })

    def _stream(self, state, completion_id, created, prompt_ids,
                max_tokens, temperature, top_p, stop_strings) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

        def send_chunk(delta: dict, finish=None) -> None:
            chunk = {
                "id": completion_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": state.model_id,
                "choices": [{
                    "index": 0,
                    "delta": delta,
                    "finish_reason": finish,
                }],
            }
            try:
                self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode("utf-8"))
                self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError) as exc:
                raise ClientGone from exc

        send_chunk({"role": "assistant", "content": ""})
        try:
            _text, finish, n_tokens = self._generate_text(
                state, prompt_ids, max_tokens, temperature, top_p, stop_strings,
                on_token=lambda d: send_chunk({"content": d}),
            )
        except ClientGone:
            # A cancelled turn must not keep the CPU busy generating tokens
            # nobody will read.
            log("client disconnected; stopped generating")
            return
        send_chunk({}, finish=finish)
        # OpenAI sends usage on the final chunk with no choices; without it
        # clients that track token accounting show nothing for local turns.
        usage = {
            "id": completion_id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": state.model_id,
            "choices": [],
            "usage": {
                "prompt_tokens": len(prompt_ids),
                "completion_tokens": n_tokens,
                "total_tokens": len(prompt_ids) + n_tokens,
            },
        }
        try:
            self.wfile.write(f"data: {json.dumps(usage)}\n\n".encode("utf-8"))
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass


def load_model(state: State, model: str, cache_dir: str) -> None:
    try:
        model_dir = resolve_model_dir(model, cache_dir)
        state.engine = OnnxEngine(model_dir)
    except Exception as exc:  # noqa: BLE001 - reported via /healthz
        state.error = str(exc)
        log(f"model load failed: {exc}")


def load_embeddings(state: State, model: str, cache_dir: str) -> None:
    try:
        model_dir = resolve_model_dir(model, cache_dir)
        state.embeddings = EmbeddingEngine(model_dir)
    except Exception as exc:  # noqa: BLE001 - reported via /healthz and /embeddings
        state.embedding_error = str(exc)
        log(f"embedding model load failed: {exc}")
    finally:
        state.embedding_loading = False


def main() -> int:
    global STATE, MAX_TOKENS
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", default=DEFAULT_MODEL,
                        help="Hugging Face repo id or local model directory")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8080)
    parser.add_argument("--cache-dir", default=default_cache_dir(),
                        help="where downloaded models are cached")
    parser.add_argument("--max-tokens", type=int, default=MAX_OUTPUT_TOKENS,
                        help="server-side cap for a single completion")
    parser.add_argument("--prewarm", action="store_true",
                        help="download the model and exit (no ONNX Runtime needed)")
    parser.add_argument("--prewarm-embeddings", action="store_true",
                        help="with --prewarm, also fetch the default embedding model")
    parser.add_argument("--embedding-model", default=None,
                        help="encoder model for /v1/embeddings "
                             "(e.g. Xenova/all-MiniLM-L6-v2); omitted = route disabled")
    args = parser.parse_args()
    MAX_TOKENS = max(1, min(args.max_tokens, MAX_OUTPUT_TOKENS))
    if args.prewarm:
        targets = [args.model]
        # Prefetching the encoder is opt-in: it is only needed for semantic
        # search, so a plain `--prewarm` should not pull two models.
        if args.prewarm_embeddings:
            targets.append(args.embedding_model or DEFAULT_EMBEDDING_MODEL)
        for target in targets:
            try:
                model_dir = prewarm(target, args.cache_dir)
            except RuntimeError as exc:
                log(f"prewarm failed for {target}: {exc}")
                return 1
            log(f"model ready in {model_dir}")
        return 0
    STATE = State(args.model, args.embedding_model)
    try:
        server = ThreadingHTTPServer((args.host, args.port), Handler)
    except OSError as exc:
        log(f"cannot listen on {args.host}:{args.port}: {exc}")
        log("another server already uses that port; stop it or choose --port")
        return 1
    loader = threading.Thread(
        target=load_model, args=(STATE, args.model, args.cache_dir), daemon=True
    )
    loader.start()
    if args.embedding_model:
        threading.Thread(
            target=load_embeddings,
            args=(STATE, args.embedding_model, args.cache_dir),
            daemon=True,
        ).start()
    log(f"listening on http://{args.host}:{args.port}/v1 (model {args.model})")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
