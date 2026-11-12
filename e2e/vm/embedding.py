"""Run a prepared local Qwen server and the small retrieval check, then stop it."""

import argparse
import json
from pathlib import Path
import sys

from e2e.cache import cache_path
from e2e.embeddings.smoke import execute
from e2e.models import qwen3
from e2e.paths import product_default, product_root, require_paths
from release.cancellation import deadline, finish
from release.files import atomic_write_json, create_directory, raw_log
from release.owned_process import spawn
from release.progress import entry


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=product_default("target/release/comb"))
    parser.add_argument("--cache", type=Path, default=cache_path())
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    config = args.cache / "comb-embedding.json"
    args.embedding_config = config
    require_paths(args, parser, "binary", "embedding_config", product="Comb")
    if args.output_dir.exists():
        parser.error("Use a new model-smoke output directory")
    create_directory(args.output_dir, mode=0o700)
    with raw_log(args.output_dir / "model.log") as log:
        with spawn(
            [
                sys.executable,
                str(product_root() / "e2e/run.py"),
                "model",
                "serve",
                "--directory",
                str(args.cache),
            ],
            stderr=log,
            limit=64 * 1024**2,
            stop_grace=240,
        ) as model:
            with deadline(qwen3.READY_SECONDS + 10):
                while True:
                    value = json.loads(model.line(timeout=qwen3.READY_SECONDS + 10))
                    if not isinstance(value, dict):
                        raise ValueError("Unexpected model readiness record")
                    if value.get("ready") is True:
                        break
            result = execute(args.binary, config)
            if model.exited():
                raise RuntimeError("The embedding model exited during the retrieval check")

    def publish():
        atomic_write_json(args.output_dir / "results.json", result)
        print("HIVE_VM_JSON " + json.dumps(result), flush=True)

    finish(publish)


if __name__ == "__main__":
    raise SystemExit(entry(main, name="verify"))
