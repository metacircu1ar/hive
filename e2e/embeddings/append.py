"""Bounded offline CLI append timing; no providers or model requests."""

from release.temporary import directory as temporary_directory
from release.process import run as run_process
from release.cancellation import check, pause, finish
from e2e.scripts.harness.hashing import sha256
import argparse
import hashlib
import json
import pathlib
import statistics
import subprocess
import tempfile
import time

from e2e.paths import product_default, product_root, require_paths
from release.progress import entry
from release.process import json_reply
from e2e.scripts.harness.checks import require


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary", type=pathlib.Path, default=product_default("target/release/comb")
    )
    parser.add_argument("--count", type=int, default=100)
    args = parser.parse_args()
    require_paths(args, parser, "binary", product="Comb")
    binary = args.binary.resolve()
    if "release" not in binary.parts or not 1 <= args.count <= 1000:
        parser.error("Use an explicit release binary and a count from 1 to 1000")
    with temporary_directory(prefix="comb-append-") as directory:

        def call(*command, body=None):
            result = run_process(
                [str(binary), "--root", directory, *command],
                input=body,
                text=True,
                capture_output=True,
                timeout=60,
                check=False,
            )
            return json_reply(result)

        call("init")
        call(
            "put",
            "components/writer",
            "--request-id",
            "create",
            body="---\ntitle: Writer\ntype: component\n---\nMeasured writer.\n",
        )
        samples = []
        start = time.monotonic()
        for i in range(args.count):
            one = time.monotonic()
            call(
                "append",
                "components/writer",
                f"Measurement {i}",
                "--source",
                "benchmark:append",
                "--request-id",
                f"append-{i}",
            )
            samples.append(time.monotonic() - one)
        elapsed = time.monotonic() - start
        document = call("get", "components/writer")
        require(len(document["page"]["timeline"]) == args.count, "Wrong persisted append count")
        ordered = sorted(samples)
        report = {
            "profile": "release",
            "binary_sha256": sha256(binary),
            "scenario": "sequential offline CLI appends; includes per-command process/open overhead",
            "count": args.count,
            "elapsed_seconds": elapsed,
            "mean_ms": statistics.mean(samples) * 1000,
            "median_ms": statistics.median(samples) * 1000,
            "p95_ms": ordered[int(0.95 * (args.count - 1))] * 1000,
            "max_ms": max(samples) * 1000,
            "verified_entries": len(document["page"]["timeline"]),
            "samples_seconds": samples,
        }
    finish(lambda: print(json.dumps(report), flush=True))


if __name__ == "__main__":
    raise SystemExit(entry(main, name="verify"))
