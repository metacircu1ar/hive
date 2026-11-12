#!/usr/bin/env python3
"""Run opt-in Hive scenarios with deterministic mock agents by default."""

import argparse
import importlib
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import e2e
from release.progress import entry, finalizer

RUNNERS = {
    ("mock", "pairs"): "run_mock_ping_pong_pair",
    ("mock", "all-to-one"): "run_mock_ping_pong_all_to_one",
    ("compare",): "run_arms",
    ("graph",): "run_comb_graph_acceptance",
    ("embeddings", "smoke"): "@embeddings.local_smoke",
    ("embeddings", "append"): "@embeddings.append",
    ("embeddings", "paired-append"): "@embeddings.paired_append",
    ("embeddings", "contract"): "@embeddings.qwen_contract",
    ("model",): "@models.qwen3",
    ("container",): "local_ci",
    ("vm",): "vm_validation",
    ("live", "controls"): "run_conversation_controls",
    ("live", "factorial"): "run_factorial_demo",
    ("live", "commit-review"): "run_commit_review_demo",
    ("live", "shop-team"): "run_shop_team",
    ("live", "comb"): "run_live_comb",
    ("live", "comb-graph"): "run_live_comb_graph",
    ("live", "comb-upkeep"): "run_live_comb_upkeep",
    ("live", "pairs"): "run_live_ping_pong_pair",
    ("live", "all-to-one"): "run_ping_pong_all_to_one",
    ("live",): "@live.runner",
    ("campaign",): "@campaign",
}


def main():
    args = sys.argv[1:] or ["mock", "pairs", "--agents", "100"]
    if args == ["embeddings"]:
        args.append("smoke")
    if args[0] == "unit":
        parser = argparse.ArgumentParser(description="Run only the opt-in suite's tests")
        parser.add_argument("--pattern", default="test_*.py")
        selected = parser.parse_args(args[1:])
        suite = unittest.defaultTestLoader.discover(
            str(Path(__file__).parent / "tests"),
            pattern=selected.pattern,
            top_level_dir=str(Path(__file__).resolve().parents[1]),
        )
        if not unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful():
            raise SystemExit(1)
        return
    if args[0] == "prepare":
        from e2e.prepare import main as prepare

        prepare(args[1:])
        return
    for command, module in RUNNERS.items():
        if args[: len(command)] == list(command):
            tail = args[len(command) :]
            if tail[:1] == ["--"]:
                tail = tail[1:]
            previous = sys.argv
            with finalizer(lambda: setattr(sys, "argv", previous)):
                sys.argv = [module, *tail]
                package = "e2e." + module[1:] if module.startswith("@") else "e2e.scripts." + module
                result = importlib.import_module(package).main()
                if result is not None:
                    raise SystemExit(result)
            return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "command",
        choices=(
            "unit",
            "mock pairs",
            "mock all-to-one",
            "compare",
            "prepare",
            "graph",
            "embeddings",
            "model",
            "container",
            "vm",
            "live",
            "live controls",
            "live factorial",
            "live commit-review",
            "live shop-team",
            "live comb",
            "live comb-graph",
            "live comb-upkeep",
            "live pairs",
            "live all-to-one",
            "campaign",
        ),
    )
    parser.parse_args(args)


if __name__ == "__main__":
    raise SystemExit(entry(main, name="scenario"))
