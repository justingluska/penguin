"""Embed the eval's passages and queries with a real model, offline.

    cargo run -p penguin-core --example hybrid_eval -- dump > /tmp/texts.json
    uv run --python 3.12 --with fastembed python embed.py /tmp/texts.json <out dir> <model>

Writes <out dir>/vectors-<model name>.json: {model, dims, passages: {text: hex},
queries: {text: hex}}, each vector L2-normalized then quantized to int8 (hex,
two's complement) to keep the file small; hybrid_eval.rs re-normalizes.
Queries and passages use the model's own query/passage encoding
(fastembed's query_embed / passage_embed, e.g. BGE's query instruction).
Nothing here runs in the app; it only makes eval fixtures.
"""

import json
import sys

import numpy as np
from fastembed import TextEmbedding


def hexq(v):
    v = np.asarray(v, dtype=np.float32)
    v = v / (np.linalg.norm(v) or 1.0)
    q = np.clip(np.round(v * 127), -127, 127).astype(np.int8)
    return q.tobytes().hex()


def main():
    texts_path, out_dir, model_name = sys.argv[1], sys.argv[2], sys.argv[3]
    texts = json.load(open(texts_path))
    model = TextEmbedding(model_name)
    slug = model_name.split("/")[-1]
    path = f"{out_dir}/vectors-{slug}.json"
    # Reuse vectors already in the output file; embed only new texts.
    try:
        old = json.load(open(path))
    except (OSError, ValueError):
        old = {"passages": {}, "queries": {}}
    passages = [t for t in texts["passages"] if t not in old["passages"]]
    queries = [t for t in texts["queries"] if t not in old["queries"]]
    pv = list(model.passage_embed(passages)) if passages else []
    qv = list(model.query_embed(queries)) if queries else []
    keep_p = set(texts["passages"])
    keep_q = set(texts["queries"])
    out = {
        "model": slug,
        "dims": len(next(iter(model.query_embed(["dims"])))),
        "passages": {t: v for t, v in old["passages"].items() if t in keep_p},
        "queries": {t: v for t, v in old["queries"].items() if t in keep_q},
    }
    out["passages"].update({t: hexq(v) for t, v in zip(passages, pv)})
    out["queries"].update({t: hexq(v) for t, v in zip(queries, qv)})
    passages, queries = out["passages"], out["queries"]
    with open(path, "w") as f:
        json.dump(out, f, sort_keys=True, separators=(",", ":"))
    print(f"wrote {path}: {len(passages)} passages, {len(queries)} queries, {out['dims']} dims")


if __name__ == "__main__":
    main()
