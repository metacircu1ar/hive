"""Balanced, interleaved release append comparisons."""

from release.temporary import directory as temporary_directory
from release.process import run as run_process
from release.cancellation import check, pause, finish
from e2e.scripts.harness.hashing import sha256
from release.files import create_directory, atomic_write_text
from release.progress import entry
from release.process import json_reply
import argparse
import hashlib
import json
from pathlib import Path
import resource
import statistics
import subprocess
import tempfile
import time

PAIRS = 5
COUNT = 100
RATIO_LIMIT = 1.05
DELTA_LIMIT_MS = 1.0


def call(binary, root, *args, body=None):
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    start = time.perf_counter()
    result = run_process(
        [str(binary), "--root", str(root), *args],
        input=body,
        text=True,
        capture_output=True,
        timeout=60,
        check=False,
    )
    wall = time.perf_counter() - start
    end = resource.getrusage(resource.RUSAGE_CHILDREN)
    value = json_reply(result)
    return value, {
        "wall_ms": wall * 1000,
        "user_ms": (end.ru_utime - usage.ru_utime) * 1000,
        "system_ms": (end.ru_stime - usage.ru_stime) * 1000,
    }


def pair(number, binaries, hashes):
    with temporary_directory(prefix="hb-pair-", parent="/tmp") as temporary:
        roots = {arm: Path(temporary) / arm for arm in binaries}
        samples = {arm: [] for arm in binaries}
        initialization = list(binaries)[:: 1 if number % 2 else -1]
        for arm in initialization:
            call(binaries[arm], roots[arm], "init")
            call(
                binaries[arm],
                roots[arm],
                "put",
                "components/writer",
                "--request-id",
                "create",
                body="---\ntitle: Writer\ntype: component\n---\nMeasured writer.\n",
            )
        for index in range(COUNT):
            order = list(binaries)[:: 1 if (index + number) % 2 else -1]
            for arm in order:
                _, timing = call(
                    binaries[arm],
                    roots[arm],
                    "append",
                    "components/writer",
                    f"Measurement {index}",
                    "--source",
                    "benchmark:append",
                    "--request-id",
                    f"append-{index}",
                )
                samples[arm].append(timing)
        for arm in binaries:
            page, _ = call(binaries[arm], roots[arm], "get", "components/writer")
            if len(page["page"]["timeline"]) != COUNT:
                raise AssertionError("Append count differs from persisted evidence")
        medians = {
            arm: {key: statistics.median(row[key] for row in rows) for key in rows[0]}
            for arm, rows in samples.items()
        }
        before, after = (medians[arm]["wall_ms"] for arm in ("before", "after"))
        return {
            "pair": number,
            "profile": "release",
            "verified_entries": COUNT,
            "binary_sha256": hashes,
            "initialization_order": initialization,
            "first_append_order": initialization,
            "alternates_each_append": True,
            "medians": medians,
            "wall_ratio": after / before,
            "wall_delta_ms": after - before,
            "samples": samples,
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--after", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    binaries = {arm: getattr(args, arm).resolve() for arm in ("before", "after")}
    if any("release" not in binary.parts or not binary.is_file() for binary in binaries.values()):
        parser.error("Both binaries must exist in explicit release paths")
    create_directory(args.output_dir)
    paths = [args.output_dir / f"paired-append-{i}.json" for i in range(1, PAIRS + 1)]
    summary_path = args.output_dir / "paired-append-summary.json"
    if any(path.exists() for path in [*paths, summary_path]):
        parser.error("Refusing to overwrite existing measurements; use a new output directory")
    hashes = {arm: sha256(binary) for arm, binary in binaries.items()}
    results = []
    for number, path in enumerate(paths, 1):
        record = pair(number, binaries, hashes)
        atomic_write_text(path, json.dumps(record, indent=2) + "\n")
        results.append(record)
        print(
            json.dumps({key: record[key] for key in ("pair", "wall_ratio", "wall_delta_ms")}),
            flush=True,
        )
    ratio = statistics.median(row["wall_ratio"] for row in results)
    delta = statistics.median(row["wall_delta_ms"] for row in results)
    regression = ratio > RATIO_LIMIT and delta > DELTA_LIMIT_MS
    summary = {
        "pairs": PAIRS,
        "entries_per_arm": COUNT,
        "median_pair_wall_ratio": ratio,
        "median_pair_wall_delta_ms": delta,
        "ratio_limit": RATIO_LIMIT,
        "delta_limit_ms": DELTA_LIMIT_MS,
        "ratio_exceeded": ratio > RATIO_LIMIT,
        "delta_exceeded": delta > DELTA_LIMIT_MS,
        "primary_gate_passed": not regression,
        "rule": "Regression requires both thresholds; secondary blocks cannot override this gate.",
    }

    def publish():
        atomic_write_text(summary_path, json.dumps(summary, indent=2) + "\n")
        print(json.dumps(summary), flush=True)
        if regression:
            raise SystemExit(1)

    finish(publish)


if __name__ == "__main__":
    raise SystemExit(entry(main, name="verify"))
