"""Tests for the termed local ONNX inference server.

Builds a tiny deterministic ONNX model (embedding + linear head) and a
small BPE tokenizer, then exercises the HTTP API end to end. No network
or model download is involved.

Run:  python3 -m unittest discover -s local_inference/tests -v
"""

import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import onnx_server  # noqa: E402


def build_tiny_model(model_dir: str) -> str:
    """Create model.onnx + tokenizer.json + configs in model_dir."""
    import numpy as np
    import onnx
    from onnx import TensorProto, helper, numpy_helper
    from tokenizers import Tokenizer, decoders, models, pre_tokenizers, trainers

    os.makedirs(model_dir, exist_ok=True)
    tok = Tokenizer(models.BPE(unk_token="[UNK]"))
    tok.pre_tokenizer = pre_tokenizers.ByteLevel(add_prefix_space=False)
    tok.decoder = decoders.ByteLevel()
    trainer = trainers.BpeTrainer(
        vocab_size=64,
        special_tokens=["[UNK]", "<|im_start|>", "<|im_end|>", "<|endoftext|>"],
    )
    tok.train_from_iterator(
        [
            "<|im_start|>user\nhello world<|im_end|>\n<|im_start|>assistant\n",
            "hello world hello",
        ],
        trainer,
    )
    tok.save(os.path.join(model_dir, "tokenizer.json"))
    vocab_size = tok.get_vocab_size()
    eos_id = tok.token_to_id("<|endoftext|>")
    assert eos_id is not None

    rng = np.random.default_rng(0)
    dim = 8
    emb = rng.normal(size=(vocab_size, dim)).astype(np.float32)
    weight = rng.normal(size=(dim, vocab_size)).astype(np.float32)
    bias = rng.normal(size=(vocab_size,)).astype(np.float32)
    graph = helper.make_graph(
        [
            helper.make_node("Gather", ["emb", "input_ids"], ["gathered"], axis=0),
            helper.make_node("MatMul", ["gathered", "weight"], ["projected"]),
            helper.make_node("Add", ["projected", "bias"], ["logits"]),
        ],
        "tiny",
        [
            helper.make_tensor_value_info(
                "input_ids", TensorProto.INT64, [1, "seq"]
            )
        ],
        [helper.make_tensor_value_info("logits", TensorProto.FLOAT, [1, "seq", vocab_size])],
        [
            numpy_helper.from_array(emb, name="emb"),
            numpy_helper.from_array(weight, name="weight"),
            numpy_helper.from_array(bias, name="bias"),
        ],
    )
    model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)])
    onnx.save(model, os.path.join(model_dir, "model.onnx"))
    with open(os.path.join(model_dir, "config.json"), "w", encoding="utf-8") as fh:
        json.dump(
            {"eos_token_id": eos_id, "num_attention_heads": 1, "hidden_size": dim},
            fh,
        )
    with open(os.path.join(model_dir, "tokenizer_config.json"), "w", encoding="utf-8") as fh:
        json.dump(
            {"chat_template": "<|im_start|>{{ m.role }}\n{{ m.content }}<|im_end|>\n"},
            fh,
        )
    return model_dir


def build_tiny_embedder(model_dir: str) -> str:
    """A 3-node encoder-shaped graph: Gather -> MatMul, mean-pooled by the
    server. Enough to prove the /embeddings route wires input to output."""
    import numpy as np
    import onnx
    from onnx import TensorProto, helper, numpy_helper
    from tokenizers import Tokenizer, decoders, models, pre_tokenizers, trainers

    os.makedirs(model_dir, exist_ok=True)
    tok = Tokenizer(models.BPE(unk_token="[UNK]"))
    tok.pre_tokenizer = pre_tokenizers.ByteLevel(add_prefix_space=False)
    tok.decoder = decoders.ByteLevel()
    tok.train_from_iterator(
        ["alpha beta gamma", "delta epsilon", "alpha alpha beta"],
        trainers.BpeTrainer(vocab_size=48, special_tokens=["[UNK]", "[PAD]"]),
    )
    tok.save(os.path.join(model_dir, "tokenizer.json"))
    vocab = tok.get_vocab_size()

    rng = np.random.default_rng(1)
    dim = 6
    table = rng.normal(size=(vocab, dim)).astype(np.float32)
    weight = np.eye(dim, dtype=np.float32)
    graph = helper.make_graph(
        [
            helper.make_node("Gather", ["table", "input_ids"], ["hidden"], axis=0),
            helper.make_node("MatMul", ["hidden", "weight"], ["last_hidden_state"]),
        ],
        "tiny-encoder",
        [
            helper.make_tensor_value_info("input_ids", TensorProto.INT64, [None, None]),
            helper.make_tensor_value_info(
                "attention_mask", TensorProto.INT64, [None, None]
            ),
        ],
        [
            helper.make_tensor_value_info(
                "last_hidden_state", TensorProto.FLOAT, [None, None, dim]
            )
        ],
        [
            numpy_helper.from_array(table, name="table"),
            numpy_helper.from_array(weight, name="weight"),
        ],
    )
    model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)])
    onnx.save(model, os.path.join(model_dir, "model.onnx"))
    return model_dir


def start_server(state):
    server = onnx_server.ThreadingHTTPServer(("127.0.0.1", 0), onnx_server.Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, server.server_address[1]


def request(port, method, path, body=None, raw=None):
    url = f"http://127.0.0.1:{port}{path}"
    data = raw if raw is not None else (
        json.dumps(body).encode("utf-8") if body is not None else None
    )
    req = urllib.request.Request(
        url, data=data, method=method,
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            return resp.status, resp.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read().decode("utf-8")


class ServerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.model_dir = build_tiny_model(os.path.join(cls.tmp.name, "model"))
        cls.state = onnx_server.State("tiny-test-model")
        onnx_server.load_model(cls.state, cls.model_dir, cls.tmp.name)
        assert cls.state.engine is not None, cls.state.error
        onnx_server.STATE = cls.state
        cls.server, cls.port = start_server(cls.state)

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.tmp.cleanup()

    def test_healthz_ok(self):
        status, body = request(self.port, "GET", "/healthz")
        self.assertEqual(status, 200)
        data = json.loads(body)
        self.assertEqual(data["status"], "ok")
        self.assertEqual(data["version"], onnx_server.__version__)

    def test_models(self):
        status, body = request(self.port, "GET", "/v1/models")
        self.assertEqual(status, 200)
        data = json.loads(body)
        self.assertEqual(data["data"][0]["id"], "tiny-test-model")

    def test_chat_completion(self):
        status, body = request(self.port, "POST", "/v1/chat/completions", {
            "model": "tiny-test-model",
            "messages": [{"role": "user", "content": "hello world"}],
            "max_tokens": 4,
            "temperature": 0,
        })
        self.assertEqual(status, 200, body)
        data = json.loads(body)
        choice = data["choices"][0]
        self.assertIsInstance(choice["message"]["content"], str)
        self.assertIn(choice["finish_reason"], ("stop", "length"))
        self.assertEqual(data["usage"]["completion_tokens"], 4)
        self.assertGreater(data["usage"]["prompt_tokens"], 0)
        # The prompt is context, never part of the reply.
        content = choice["message"]["content"]
        self.assertNotIn("hello world", content)
        self.assertNotIn("im_start", content)

    def test_chat_completion_is_deterministic_when_greedy(self):
        payload = {
            "messages": [{"role": "user", "content": "hello world"}],
            "max_tokens": 4,
            "temperature": 0,
        }
        _, first = request(self.port, "POST", "/v1/chat/completions", payload)
        _, second = request(self.port, "POST", "/v1/chat/completions", payload)
        self.assertEqual(
            json.loads(first)["choices"][0]["message"]["content"],
            json.loads(second)["choices"][0]["message"]["content"],
        )

    def test_chat_completion_streaming(self):
        status, body = request(self.port, "POST", "/v1/chat/completions", {
            "messages": [{"role": "user", "content": "hello world"}],
            "max_tokens": 4,
            "temperature": 0,
            "stream": True,
        })
        self.assertEqual(status, 200, body)
        self.assertIn("data: [DONE]", body)
        chunks = [
            json.loads(line[6:])
            for line in body.splitlines()
            if line.startswith("data: ") and line != "data: [DONE]"
        ]
        self.assertTrue(chunks)
        self.assertEqual(chunks[0]["choices"][0]["delta"].get("role"), "assistant")
        finishes = [
            c["choices"][0]["finish_reason"] for c in chunks if c["choices"]
        ]
        self.assertIn("length", finishes)
        streamed = "".join(
            c["choices"][0]["delta"].get("content", "")
            for c in chunks
            if c["choices"]
        )
        self.assertNotIn("hello world", streamed)
        self.assertNotIn("im_start", streamed)
        usage = [c for c in chunks if c.get("usage")][-1]["usage"]
        self.assertEqual(usage["completion_tokens"], 4)
        self.assertGreater(usage["prompt_tokens"], 0)

    def test_disconnect_mid_stream_stops_generation(self):
        # A cancelled turn closes the socket; the server must stop cleanly
        # (no traceback) and keep serving later requests.
        code = (
            "import socket, json\n"
            "s = socket.create_connection(('127.0.0.1', %d))\n"
            "body = json.dumps({'messages':[{'role':'user','content':'hello'}],"
            "'max_tokens':200,'stream':True}).encode()\n"
            "head = (b'POST /v1/chat/completions HTTP/1.1\\r\\n'\n"
            "        b'Host: x\\r\\nContent-Type: application/json\\r\\n'\n"
            "        b'Content-Length: ' + str(len(body)).encode() + b'\\r\\n\\r\\n')\n"
            "s.sendall(head + body)\n"
            "s.recv(64)\n"
            "s.close()\n"
        ) % self.port
        subprocess.run([sys.executable, "-c", code], check=True, timeout=30)
        # Still answering after the aborted stream.
        status, body = request(self.port, "POST", "/v1/chat/completions", {
            "messages": [{"role": "user", "content": "hello"}],
            "max_tokens": 2,
            "temperature": 0,
        })
        self.assertEqual(status, 200, body)

    def test_shutdown_accepts_a_body(self):
        # ureq 3.x bodyless POSTs use send("{}"), so the route must not care
        # whether a body is present.
        state = onnx_server.State("shutdown-body-model")
        onnx_server.STATE = state
        server, port = start_server(state)
        try:
            result = {}
            def call():
                result["response"] = request(
                    port, "POST", "/admin/shutdown", raw=b"{}"
                )
            worker = threading.Thread(target=call)
            worker.start()
            worker.join(timeout=10)
            status, body = result["response"]
            self.assertEqual(status, 200, body)
            self.assertEqual(json.loads(body)["status"], "shutting_down")
        finally:
            onnx_server.STATE = self.state

    def test_tools_field_is_accepted_and_ignored(self):
        status, body = request(self.port, "POST", "/v1/chat/completions", {
            "messages": [{"role": "user", "content": "hello world"}],
            "max_tokens": 2,
            "temperature": 0,
            "tools": [{"type": "function", "function": {"name": "noop"}}],
        })
        self.assertEqual(status, 200, body)

    def test_invalid_json_is_400(self):
        status, _ = request(self.port, "POST", "/v1/chat/completions", raw=b"{nope")
        self.assertEqual(status, 400)

    def test_missing_messages_is_400(self):
        status, _ = request(self.port, "POST", "/v1/chat/completions", {"model": "x"})
        self.assertEqual(status, 400)

    def test_unknown_route_is_404(self):
        status, _ = request(self.port, "GET", "/nope")
        self.assertEqual(status, 404)

    def test_loading_state_is_503(self):
        loading = onnx_server.State("loading-model")
        onnx_server.STATE = loading
        server, port = start_server(loading)
        try:
            status, body = request(port, "GET", "/healthz")
            self.assertEqual(status, 503)
            self.assertEqual(json.loads(body)["status"], "loading")
            status, _ = request(port, "POST", "/v1/chat/completions", {
                "messages": [{"role": "user", "content": "hi"}],
            })
            self.assertEqual(status, 503)
        finally:
            server.shutdown()
            server.server_close()
            onnx_server.STATE = self.state

    def test_shutdown_endpoint(self):
        state = onnx_server.State("shutdown-model")
        onnx_server.STATE = state
        server, port = start_server(state)
        try:
            # Shut down from a thread: /admin/shutdown itself runs the server
            # loop shutdown, which must not be awaited on its own thread.
            body = {}
            def call():
                body["response"] = request(port, "POST", "/admin/shutdown")
            worker = threading.Thread(target=call)
            worker.start()
            worker.join(timeout=10)
            status, payload = body["response"]
            self.assertEqual(status, 200)
            self.assertEqual(json.loads(payload)["status"], "shutting_down")
        finally:
            server.shutdown()
            server.server_close()
            onnx_server.STATE = self.state


class HubRetryTests(unittest.TestCase):
    """A reset connection must not abort the local fallback.

    Reproduces the Windows failure (`WinError 10054`, "existing connection was
    forcibly closed") where one dropped TLS handshake made every 503 fallback
    fail before the model was ever fetched.
    """

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.cache = self.tmp.name
        self.repo = "owner/termed-retry-test"
        self.calls = 0
        self.reset_attempts = 1
        self.original_hub = onnx_server.HUB
        self.original_sleep = onnx_server.time.sleep
        onnx_server.time.sleep = lambda _s: None  # keep the test instant

        server = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                server.calls += 1
                if server.reset_attempts > 0:
                    server.reset_attempts -= 1
                    # Drop the connection without a response.
                    self.close_connection = True
                    self.connection.close()
                    return
                path = self.path
                if "does-not-exist" in path:
                    # A definitive 404: the repo is not on the Hub.
                    self.send_error(404)
                    return
                if path.startswith("/api/models/"):
                    body = json.dumps({"siblings": [
                        {"rfilename": "onnx/model.onnx"},
                        {"rfilename": "tokenizer.json"},
                        {"rfilename": "README.md"},
                    ]}).encode()
                elif path.endswith("/resolve/main/onnx/model.onnx"):
                    body = b"fake-onnx-bytes"
                elif path.endswith("/resolve/main/tokenizer.json"):
                    body = b"{}"
                else:
                    self.send_error(404)
                    return
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        port = self.server.server_address[1]
        onnx_server.HUB = f"http://127.0.0.1:{port}"

    def tearDown(self):
        onnx_server.HUB = self.original_hub
        onnx_server.time.sleep = self.original_sleep
        self.server.shutdown()
        self.server.server_close()
        self.tmp.cleanup()

    def test_a_dropped_connection_is_retried_instead_of_failing(self):
        model_dir = onnx_server.resolve_model_dir(self.repo, self.cache)
        # One reset, then a successful listing and downloads.
        self.assertEqual(self.reset_attempts, 0)
        self.assertEqual(
            os.path.basename(model_dir), self.repo.replace("/", "--")
        )
        self.assertTrue(
            os.path.isfile(os.path.join(model_dir, "onnx", "model.onnx"))
        )
        self.assertTrue(os.path.isfile(os.path.join(model_dir, "tokenizer.json")))
        # Unlisted files are never fetched.
        self.assertFalse(os.path.isfile(os.path.join(model_dir, "README.md")))
        # No leftover partial downloads.
        partials = [
            name
            for _root, _dirs, files in os.walk(model_dir)
            for name in files
            if name.endswith(".part")
        ]
        self.assertEqual(partials, [])

    def test_persistent_failure_explains_the_mirror_and_offline_options(self):
        self.reset_attempts = 99
        with self.assertRaises(RuntimeError) as error:
            onnx_server.resolve_model_dir(self.repo, self.cache)
        message = str(error.exception)
        self.assertIn("after 4 attempts", message)
        self.assertIn("HF_ENDPOINT", message)
        self.assertIn("--model", message)
        # It retried rather than failing on the first reset.
        self.assertGreaterEqual(self.calls, 4)

    def test_cached_model_needs_no_network(self):
        onnx_server.resolve_model_dir(self.repo, self.cache)
        before = self.calls
        onnx_server.HUB = "http://127.0.0.1:9"  # unroutable: must not be used
        again = onnx_server.resolve_model_dir(self.repo, self.cache)
        self.assertEqual(again, os.path.join(
            self.cache, self.repo.replace("/", "--")))
        self.assertEqual(self.calls, before)

    def test_a_definitive_4xx_answer_fails_fast_without_retrying(self):
        # A 404 answers in one request, with no backoff attempts.
        self.reset_attempts = 0
        with self.assertRaises(RuntimeError) as error:
            onnx_server.resolve_model_dir("owner/does-not-exist", self.cache)
        self.assertIn("HTTP 404", str(error.exception))
        self.assertEqual(self.calls, 1)


class PrewarmTests(unittest.TestCase):
    """Prewarming fetches the model without needing ONNX Runtime."""

    def test_prewarm_a_local_directory_needs_no_runtime(self):
        with tempfile.TemporaryDirectory() as tmp:
            model = build_tiny_model(os.path.join(tmp, "m"))
            self.assertEqual(onnx_server.prewarm(model, tmp), model)

    def test_prewarm_rejects_a_directory_without_a_tokenizer(self):
        with tempfile.TemporaryDirectory() as tmp:
            open(os.path.join(tmp, "model.onnx"), "wb").close()
            with self.assertRaises(RuntimeError) as error:
                onnx_server.prewarm(tmp, tmp)
            self.assertIn("tokenizer.json", str(error.exception))


class EmbeddingTests(unittest.TestCase):
    """`POST /v1/embeddings`: batching, shape, and the failure modes."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.model = build_tiny_model(os.path.join(self.tmp, "chat"))
        self.saved = onnx_server.STATE
        self.state = onnx_server.State(self.model)
        self.state.engine = onnx_server.OnnxEngine(self.model)
        # The handler reads the module global, so the fixture must publish it.
        onnx_server.STATE = self.state

    def tearDown(self):
        onnx_server.STATE = self.saved
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_batches_return_one_normalised_vector_each(self):
        self.state.embedding_id = "tiny-encoder"
        self.state.embeddings = onnx_server.EmbeddingEngine(
            build_tiny_embedder(os.path.join(self.tmp, "emb"))
        )
        server, port = start_server(self.state)
        try:
            status, body = request(
                port, "POST", "/v1/embeddings", {"input": ["alpha beta", "gamma"]}
            )
            self.assertEqual(status, 200, body)
            payload = json.loads(body)
            self.assertEqual(len(payload["data"]), 2)
            self.assertEqual(payload["model"], "tiny-encoder")
            for item in payload["data"]:
                vector = item["embedding"]
                self.assertEqual(len(vector), 6)
                norm = math.sqrt(sum(v * v for v in vector))
                self.assertAlmostEqual(norm, 1.0, places=4)
            # Indexes identify which input each vector belongs to.
            self.assertEqual([i["index"] for i in payload["data"]], [0, 1])
        finally:
            server.shutdown()
            server.server_close()

    def test_a_single_string_is_accepted(self):
        self.state.embedding_id = "tiny-encoder"
        self.state.embeddings = onnx_server.EmbeddingEngine(
            build_tiny_embedder(os.path.join(self.tmp, "emb2"))
        )
        server, port = start_server(self.state)
        try:
            status, body = request(port, "POST", "/v1/embeddings", {"input": "alpha"})
            self.assertEqual(status, 200, body)
            self.assertEqual(len(json.loads(body)["data"]), 1)
        finally:
            server.shutdown()
            server.server_close()

    def test_no_model_is_503_and_says_how_to_enable_it(self):
        server, port = start_server(self.state)   # embedding_id is None
        try:
            status, body = request(port, "POST", "/v1/embeddings", {"input": "x"})
            self.assertEqual(status, 503, body)
            self.assertIn("--embedding-model", body)
        finally:
            server.shutdown()
            server.server_close()

    def test_bad_inputs_are_400(self):
        self.state.embedding_id = "tiny-encoder"
        self.state.embeddings = onnx_server.EmbeddingEngine(
            build_tiny_embedder(os.path.join(self.tmp, "emb3"))
        )
        server, port = start_server(self.state)
        try:
            for body in ({"input": []}, {"input": [1, 2]}, {"input": 5}, {}):
                status, text = request(port, "POST", "/v1/embeddings", body)
                self.assertEqual(status, 400, f"{body} -> {text}")
        finally:
            server.shutdown()
            server.server_close()

    def test_health_reports_embeddings_without_failing_chat(self):
        self.state.embedding_id = "tiny-encoder"
        self.state.embedding_error = "disk on fire"
        server, port = start_server(self.state)
        try:
            status, body = request(port, "GET", "/healthz")
            self.assertEqual(status, 200, "chat still works")
            payload = json.loads(body)
            self.assertEqual(payload["status"], "ok")
            self.assertEqual(payload["embeddings"], "failed")
            self.assertEqual(payload["embedding_model"], "tiny-encoder")
        finally:
            server.shutdown()
            server.server_close()


class UnitTests(unittest.TestCase):
    def test_render_chatml(self):
        out = onnx_server.render_chat(
            [
                {"role": "system", "content": "be brief"},
                {"role": "user", "content": "hi"},
            ],
            "<|im_start|>{{ m.role }}",
        )
        self.assertEqual(
            out,
            "<|im_start|>system\nbe brief<|im_end|>\n"
            "<|im_start|>user\nhi<|im_end|>\n"
            "<|im_start|>assistant\n",
        )

    def test_render_plain_fallback(self):
        out = onnx_server.render_chat(
            [{"role": "user", "content": "hi"}], None
        )
        self.assertEqual(out, "user: hi\nassistant:")

    def test_find_onnx_file_layouts(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertIsNone(onnx_server.find_onnx_file(tmp))
            os.makedirs(os.path.join(tmp, "onnx"))
            path = os.path.join(tmp, "onnx", "model.onnx")
            open(path, "wb").close()
            self.assertEqual(onnx_server.find_onnx_file(tmp), path)

    def test_resolve_local_dir(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(onnx_server.resolve_model_dir(tmp, tmp), tmp)

    def test_missing_model_paths_are_not_treated_as_hub_repos(self):
        with tempfile.TemporaryDirectory() as tmp:
            # `owner/name` is ambiguous (both a repo id and a relative path),
            # so only clearly path-shaped values get the local-path error.
            for missing in ("/nonexistent/model", "./missing", "../up", "~/onnx"):
                with self.assertRaises(RuntimeError) as error:
                    onnx_server.resolve_model_dir(missing, tmp)
                self.assertIn("does not exist", str(error.exception))
            # Malformed repo ids are rejected before any network access.
            for malformed in ("smollm", "owner/name/extra", "two words"):
                with self.assertRaises(RuntimeError) as error:
                    onnx_server.resolve_model_dir(malformed, tmp)
                self.assertIn("not a Hugging Face repo id", str(error.exception))

    def test_path_like_detects_directories_and_drive_letters(self):
        self.assertTrue(onnx_server.path_like("/models/qwen"))
        self.assertTrue(onnx_server.path_like("./models/qwen"))
        self.assertTrue(onnx_server.path_like("C:\\models\\qwen"))
        self.assertTrue(onnx_server.path_like("~/onnx"))
        self.assertFalse(onnx_server.path_like("onnx-community/SmolLM2-135M-Instruct"))

    def test_pair_kv_cache_matches_present_outputs_to_past_key_values_inputs(self):
        past_names = [
            "past_key_values.0.key",
            "past_key_values.0.value",
            "past_key_values.1.key",
            "past_key_values.1.value",
        ]
        present_outputs = [
            "logits",
            "present.0.key",
            "present.0.value",
            "present.1.key",
            "present.1.value",
        ]
        self.assertEqual(
            onnx_server._pair_kv_cache(past_names, present_outputs),
            [
                ("past_key_values.0.key", "present.0.key"),
                ("past_key_values.0.value", "present.0.value"),
                ("past_key_values.1.key", "present.1.key"),
                ("past_key_values.1.value", "present.1.value"),
            ],
        )

    def test_select_onnx_files_picks_one_preferred_variant_and_its_data(self):
        names = [
            "onnx/model.onnx",
            "onnx/model.onnx_data",
            "onnx/model_fp16.onnx",
            "onnx/model_quantized.onnx",
            "onnx/model_quantized.onnx.data",
            "onnx/model_q4.onnx",
        ]
        self.assertEqual(
            onnx_server._select_onnx_files(names),
            {"onnx/model_quantized.onnx", "onnx/model_quantized.onnx.data"},
        )

    def test_onnx_community_repo_falls_back_to_suffixed_hub_repo_on_401(self):
        candidates = onnx_server._hf_repo_candidates(
            "onnx-community/SmolLM2-135M-Instruct"
        )
        self.assertIn("onnx-community/SmolLM2-135M-Instruct-ONNX", candidates)
        self.assertIn("HuggingFaceTB/SmolLM2-135M-Instruct", candidates)

    def test_render_chat_disables_thinking_for_hybrid_reasoning_templates(self):
        out = onnx_server.render_chat(
            [
                {"role": "system", "content": "be brief"},
                {"role": "user", "content": "hi"},
            ],
            "<|im_start|>{{ m.role }}\n<think></think>",
        )
        self.assertIn("/no_think\nbe brief", out)
        self.assertTrue(
            out.endswith("<|im_start|>assistant\n<think>\n\n</think>\n\n"),
            out,
        )

    def test_generate_text_stops_before_hallucinated_user_turn_and_strips_think(self):
        pieces = [
            "<think>",
            "internal monologue",
            "</think>\n",
            "Real ",
            "answer.",
            "\n",
            "user",
            ":",
            " fake follow-up question",
        ]

        class FakeEngine:
            def generate_tokens(self, _prompt_ids, max_tokens, _temp, _top_p):
                for i in range(min(max_tokens, len(pieces))):
                    yield i

            def decode(self, ids):
                return "".join(pieces[i] for i in ids)

        state = onnx_server.State("fake")
        state.engine = FakeEngine()
        streamed: list[str] = []
        handler = onnx_server.Handler.__new__(onnx_server.Handler)
        text, finish, count = handler._generate_text(
            state,
            [1, 2],
            50,
            0.0,
            1.0,
            [],
            on_token=streamed.append,
        )
        self.assertEqual(text, "Real answer.")
        self.assertEqual("".join(streamed), "Real answer.")
        self.assertEqual(finish, "stop")
        # Generation stopped at ':' (index 7 -> 8 tokens), never running the tail.
        self.assertEqual(count, 8)

    def test_render_chat_formats_tool_calls_and_tool_responses(self):
        out = onnx_server.render_chat(
            [
                {"role": "system", "content": "sys"},
                {"role": "user", "content": "read readme"},
                {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "function": {
                                "name": "read_file",
                                "arguments": '{"path":"ffmpeg.readme.md"}',
                            },
                        }
                    ],
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": "# FFmpeg README",
                },
            ],
            "",
        )
        self.assertIn(
            '<tool_call>{"name": "read_file", "arguments": {"path": "ffmpeg.readme.md"}}</tool_call>',
            out,
        )
        self.assertIn("<tool_response>\n# FFmpeg README\n</tool_response>", out)

    def test_generate_text_stops_after_closing_tool_call_tag(self):
        pieces = [
            '<tool_call>{"name": "read_file", ',
            '"arguments": {"path": "ffmpeg.readme.md"}}',
            "</tool_call>",
            "\nhallucinated trailing text",
        ]

        class FakeEngine:
            def generate_tokens(self, _prompt_ids, max_tokens, _temp, _top_p):
                for i in range(min(max_tokens, len(pieces))):
                    yield i

            def decode(self, ids):
                return "".join(pieces[i] for i in ids)

        state = onnx_server.State("fake")
        state.engine = FakeEngine()
        handler = onnx_server.Handler.__new__(onnx_server.Handler)
        text, finish, count = handler._generate_text(
            state, [1], 20, 0.0, 1.0, []
        )
        self.assertEqual(
            text,
            '<tool_call>{"name": "read_file", "arguments": {"path": "ffmpeg.readme.md"}}</tool_call>',
        )
        self.assertEqual(finish, "stop")
        self.assertEqual(count, 3)


if __name__ == "__main__":
    unittest.main()
