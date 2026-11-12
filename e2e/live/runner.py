"""The small live set runs serially, with a shared one-hour work deadline."""

import argparse
import json
import math
from pathlib import Path
import sys
import time

from e2e.live.scenario import checked, options
from e2e.paths import product_root
from release.cancellation import check, finish
from release.files import atomic_write_json, create_directory, raw_log
from release.process import run
from release.progress import finalizer, phase

CHECKS = (
    ("shop-team", 1800, "result.json", "result"),
    ("pairs", 600, "verification.json", "result"),
    ("all-to-one", 600, "verification.json", "result"),
    ("controls", 600, "result.json", "result"),
    ("comb", 600, "result.json", "status"),
    ("comb-graph", 600, "result.json", "status"),
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    options(parser, cap=3600)
    parser.add_argument(
        "--embedding-config", type=Path, default=product_root() / "examples/qwen3-embedding.json"
    )
    args = parser.parse_args()
    checked(
        parser,
        args,
        label="set",
        agents="six serial checks; at most 4 simultaneous",
        models="Codex Terra 6.1 low/medium; shop reviewer Sonnet low",
        cap=3600,
    )
    output = args.output_dir.resolve()
    create_directory(output, mode=0o700)
    results = []
    until = time.monotonic() + args.timeout

    def retain():
        if len(results) != len(CHECKS):
            atomic_write_json(
                output / "retained.json",
                dict(
                    result="unaccepted",
                    finished=results,
                    unfinished=[row[0] for row in CHECKS[len(results) :]],
                ),
            )

    with finalizer(retain):
        for name, cap, report, verdict in CHECKS:
            check()
            remaining = min(cap, math.floor(until - time.monotonic()))
            if remaining < 1:
                raise TimeoutError("The live set reached its one-hour work limit")
            command = [
                sys.executable,
                str(product_root() / "e2e/run.py"),
                "live",
                name,
                "--live",
                "--binary",
                str(args.binary),
                "--state-dir",
                str(args.state_dir),
                "--output-dir",
                str(output / name),
                "--timeout",
                str(remaining),
            ]
            if name in ("comb", "comb-graph"):
                command += ["--embedding-config", str(args.embedding_config)]
            with phase("scenario"), raw_log(output / (name + ".log")) as log:
                # This child owns hosted cleanup plus both service groups. Its
                # work deadline excludes that tail; an enclosing cancellation
                # leaves it time to finish 90s hosted cleanup, service graces and process-absence checks.
                run(
                    command,
                    timeout=remaining + 180,
                    stop_grace=180,
                    check=True,
                    stdout=log,
                    stderr=log,
                    limit=64 * 1024**2,
                )
            value = json.loads((output / name / report).read_text())
            if value[verdict] != "passed":
                raise RuntimeError("A live leaf has no passing acceptance report")
            results.append(dict(check=name, report=report))
        finish(
            lambda: atomic_write_json(output / "result.json", dict(result="passed", checks=results))
        )
