"""Tests for the termed local ONNX inference server.

Builds a tiny deterministic ONNX model (embedding + linear head) and a
small BPE tokenizer, then exercises the HTTP API end to end. No network
or model download is involved.

Run:  python3 -m unittest discover -s local_inference/tests -v
"""

import json
import os
import subprocess
import sys
import tempfile
import threading
import unittest
import urllib.error
import urllib.request

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
            status, body = request(port, "POST", "/admin/shutdown")
            self.assertEqual(status, 200)
            self.assertEqual(json.loads(body)["status"], "shutting_down")
        finally:
            server.shutdown()
            server.server_close()
            onnx_server.STATE = self.state


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


if __name__ == "__main__":
    unittest.main()
