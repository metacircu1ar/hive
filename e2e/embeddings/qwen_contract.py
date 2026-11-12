"""Check the real local Qwen endpoint: shape, retrieval, batching and long inputs."""

from release.process import run as run_process
from release.cancellation import check, pause, finish
from e2e.scripts.harness.hashing import sha256
import argparse
import json
import math
from pathlib import Path
import time
import urllib.error
import urllib.request

from e2e.paths import product_default, product_root, require_paths
from e2e.scripts.harness.checks import require
from release.progress import entry


from e2e.embeddings.http import post, HTTPFailure


def vectors(config, texts):
    response = post(
        config["url"],
        dict(
            model=config["model"],
            input=texts,
            encoding_format="float",
            dimensions=config["dimensions"],
        ),
    )
    data = sorted(response["data"], key=lambda item: item["index"])
    require(
        all(type(item["index"]) is int for item in data)
        and [item["index"] for item in data] == list(range(len(texts))),
        "Wrong embedding indices",
    )
    result = [item["embedding"] for item in data]
    for vector in result:
        require(len(vector) == config["dimensions"] == 4096, "Wrong embedding dimensions")
        require(
            all(type(x) in (int, float) and math.isfinite(x) for x in vector),
            "Nonfinite or nonnumeric embedding",
        )
        require(
            abs(math.sqrt(sum(x * x for x in vector)) - 1) < 1e-4, "Embedding is not normalized"
        )
    return result


def cosine(a, b):
    return sum(x * y for x, y in zip(a, b)) / math.sqrt(
        sum(x * x for x in a) * sum(y * y for y in b)
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--embedding-config", type=Path, default=product_default("examples/qwen3-embedding.json")
    )
    args = parser.parse_args()
    require_paths(args, parser, "embedding_config")
    config = json.loads(args.embedding_config.read_text())
    require(
        config["backend"] == "http" and config["document_prefix"] == "",
        "Wrong Qwen document configuration",
    )
    require(
        config["query_prefix"].startswith("Instruct: ") and "\nQuery:" in config["query_prefix"],
        "Wrong Qwen query configuration",
    )
    started = time.monotonic()
    documents = [
        "Paris is the capital of France.",
        "Beijing is the capital of China.",
        "A database commit preserves messages across a machine restart.",
    ]
    docs = vectors(config, documents)
    queries = [
        "What is the capital of France?",
        "中国的首都是哪里？",
        "How can saved messages survive a crash?",
    ]
    found = vectors(config, [config["query_prefix"] + query for query in queries])
    matches = [max(range(len(docs)), key=lambda j: cosine(vector, docs[j])) for vector in found]
    require(matches == [0, 1, 2], "Semantic retrieval mismatch")
    alone = vectors(config, documents[:1])[0]
    reordered = vectors(config, [documents[2] * 8, documents[0]])[1]
    parity = [cosine(docs[0], alone), cosine(docs[0], reordered)]
    require(min(parity) >= 0.9999, "Batch vector parity mismatch")
    # Long input within the configured embedding slot.
    vectors(config, ["A newly collected observation updates the page summary. " * 100])
    try:
        vectors(config, ["0123456789" * 6000])
    except HTTPFailure as error:
        rejection = dict(status=error.status, body=error.body)
        require(error.status in (400, 413, 422), "Unexpected rejection status")
    else:
        raise AssertionError("Overlong input was silently accepted/truncated")
    report = dict(
        status="passed",
        model=config["model"],
        revision=config["revision"],
        dimensions=4096,
        retrieval_matches=matches,
        batch_cosine=parity,
        long_input_accepted=True,
        overlong_input_rejected=rejection,
        elapsed_seconds=time.monotonic() - started,
    )
    finish(lambda: print(json.dumps(report), flush=True))


if __name__ == "__main__":
    raise SystemExit(entry(main, name="verify"))
