#!/usr/bin/env python3
"""Build a tiny deterministic ONNX model for termed's local-inference tests.

The graph is embedding + linear head (no attention), so it loads in
milliseconds and needs no download. Greedy decoding always picks the token
for the word "hello", which makes the output a predictable, non-empty run of
that word instead of random special tokens.

Usage: python3 build_tiny_onnx.py OUT_DIR
"""

import json
import os
import sys

CHAT_TEMPLATE = "<|im_start|>{{ m.role }}\n{{ m.content }}<|im_end|>\n"
SPECIAL = ["[UNK]", "<|im_start|>", "<|im_end|>", "<|endoftext|>"]
CORPUS = [
    "<|im_start|>user\nhello local model<|im_end|>\n"
    "<|im_start|>assistant\nhello world<|im_end|>\n",
    "hello local model hello world",
]


def build_model(out: str, vocab_size: int = 96, dim: int = 8) -> str:
    import numpy as np
    import onnx
    from onnx import TensorProto, helper, numpy_helper
    from tokenizers import Tokenizer, decoders, models, pre_tokenizers, trainers

    os.makedirs(out, exist_ok=True)
    tok = Tokenizer(models.BPE(unk_token="[UNK]"))
    tok.pre_tokenizer = pre_tokenizers.ByteLevel(add_prefix_space=False)
    tok.decoder = decoders.ByteLevel()
    tok.train_from_iterator(
        CORPUS, trainers.BpeTrainer(vocab_size=vocab_size, special_tokens=SPECIAL)
    )
    tok.save(os.path.join(out, "tokenizer.json"))

    vocab = tok.get_vocab_size()
    eos = tok.token_to_id("<|endoftext|>")
    hello = tok.token_to_id("hello")
    assert eos is not None and hello is not None

    rng = np.random.default_rng(0)
    emb = rng.normal(size=(vocab, dim)).astype(np.float32)
    weight = rng.normal(size=(dim, vocab)).astype(np.float32)
    bias = rng.normal(size=(vocab,)).astype(np.float32)
    # Greedy decoding then repeats a plain word, never a special token.
    bias[hello] += 60.0

    graph = helper.make_graph(
        [
            helper.make_node("Gather", ["emb", "input_ids"], ["gathered"], axis=0),
            helper.make_node("MatMul", ["gathered", "weight"], ["projected"]),
            helper.make_node("Add", ["projected", "bias"], ["logits"]),
        ],
        "termed-tiny",
        # Real exports also take an attention mask; the server feeds one, so
        # the fixture exposes it too.
        [
            helper.make_tensor_value_info("input_ids", TensorProto.INT64, [1, "seq"]),
            helper.make_tensor_value_info("attention_mask", TensorProto.INT64, [1, "seq"]),
        ],
        [helper.make_tensor_value_info("logits", TensorProto.FLOAT, [1, "seq", vocab])],
        [
            numpy_helper.from_array(emb, name="emb"),
            numpy_helper.from_array(weight, name="weight"),
            numpy_helper.from_array(bias, name="bias"),
        ],
    )
    onnx.save(
        helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)]),
        os.path.join(out, "model.onnx"),
    )
    with open(os.path.join(out, "config.json"), "w", encoding="utf-8") as fh:
        json.dump({"eos_token_id": eos, "num_attention_heads": 1, "hidden_size": dim}, fh)
    with open(os.path.join(out, "tokenizer_config.json"), "w", encoding="utf-8") as fh:
        json.dump({"chat_template": CHAT_TEMPLATE}, fh)
    return out


if __name__ == "__main__":
    build_model(sys.argv[1])
