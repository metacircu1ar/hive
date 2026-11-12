from release.process import run as run_process
from release.cancellation import check, pause
from e2e.scripts.harness.hashing import sha256
import sys
import unittest
from pathlib import Path
import io
import json
import tempfile
from contextlib import ExitStack, redirect_stdout
from types import SimpleNamespace
from unittest.mock import patch

from e2e.scripts import run_comb_graph_acceptance as driver
from e2e.scripts.run_comb_graph_acceptance import ordinary_cases, percentile, timings
from e2e.scripts.comb_graph_tuning import cases as tuning_cases, rank_metrics


def scale_fixture(pages):
    return dict(
        complete=True,
        incomplete_reasons=[],
        work={},
        coverage={},
        nodes=[dict(slug=f"concepts/p{i:05d}") for i in range(min(pages, 21))],
        relations=[
            dict(
                source=f"concepts/p{i:05d}",
                predicate="depends_on",
                target=f"concepts/p{(i + step) % pages:05d}",
            )
            for i in range(11)
            for step in range(1, 11)
        ],
    )


class GraphAcceptanceDriver(unittest.TestCase):
    def test_cli_propagates_failure_through_actual_progress_wrapper(self):
        with tempfile.TemporaryDirectory() as directory:
            args = SimpleNamespace(
                baseline=Path("/tmp/release/baseline"),
                binary=Path("/tmp/release/comb"),
                output_dir=Path(directory) / "output",
                pages=10_000,
            )
            with (
                patch("argparse.ArgumentParser.parse_args", return_value=args),
                patch.object(driver, "require_paths"),
                patch.object(driver, "execute", return_value=1) as execute,
            ):
                self.assertEqual(driver.cli(), 1)
                execute.assert_called_once_with(args)

    def test_failed_gate_exception_and_reduced_workload_leave_failed_receipts(self):
        for kind in ("gate", "exception", "reduced"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as directory:
                args = SimpleNamespace(
                    baseline=Path("baseline"),
                    binary=Path("current"),
                    output_dir=Path(directory),
                    pages=11 if kind == "reduced" else 10_000,
                )
                with (
                    patch.object(driver, "digest", return_value="digest"),
                    patch.object(driver, "regression", return_value=["O01"]),
                    patch.object(driver, "tuning", return_value={}),
                    patch.object(driver, "scale") as scale,
                    redirect_stdout(io.StringIO()),
                ):
                    scale.return_value = dict(
                        graph_p95_gate=kind != "gate", ordinary_regression_gate=True
                    )
                    if kind == "exception":
                        scale.side_effect = RuntimeError("original comparison failure")
                        with self.assertRaisesRegex(RuntimeError, "original comparison failure"):
                            driver.execute(args)
                    else:
                        self.assertEqual(driver.execute(args), 1)
                receipt = json.loads((args.output_dir / "result.json").read_text())
                self.assertEqual(receipt["status"], "failed")
                self.assertEqual(receipt["qualifying_size"], kind != "reduced")

    def test_deadline_miss_is_retained_and_fails_gate_without_losing_measurements(self):
        class Peer:
            def __init__(self, binary, directory, socket):
                self.last_elapsed_ms = 0.2
                self.graph_calls = 0

            def page(self, *args, **kwargs):
                pass

            def cli(self, *args):
                pass

            def start(self, *args):
                pass

            def rss(self):
                return 4096

            def result(self, operation):
                if operation["op"] == "stats":
                    return dict(pages=11)
                if operation["op"] == "doctor":
                    return dict(
                        graph=dict(
                            current_reviewed=0, current_unreviewed=110, current_incomplete_sources=0
                        )
                    )
                if operation["op"] == "search":
                    return dict(results=[dict(slug=str(i)) for i in range(10)])
                self.graph_calls += 1
                report = scale_fixture(11)
                if self.graph_calls == 1:
                    report.update(complete=False, incomplete_reasons=["deadline"])
                return report

        with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
            args = SimpleNamespace(
                baseline=Path("baseline"),
                binary=Path("current"),
                output_dir=Path(directory),
                pages=11,
            )
            with patch.object(driver, "Peer", Peer):
                result = driver.scale(args, stack, Path(directory))
            self.assertFalse(result["graph_p95_gate"])
            self.assertEqual(result["graph_incomplete_count"], 1)
            self.assertEqual(len(result["graph"]["samples_ms"]), 10)
            self.assertEqual(len(result["graph_observations"]), 13)
            saved = json.loads((Path(directory) / "scale-samples.json").read_text())
            self.assertEqual(saved["graph"][0]["reasons"], ["deadline"])

    def test_empty_and_missing_trust_participants_are_not_accepted(self):
        with self.assertRaises(RuntimeError):
            driver.ordinary_control("O01", dict(status="ok", result=dict(results=[])))
        with self.assertRaises(RuntimeError):
            driver.ordinary_control(
                "O07",
                dict(status="ok", result=dict(results=[dict(slug="concepts/a", unverified=False)])),
            )
        with self.assertRaises(RuntimeError):
            driver.ordinary_control(
                "O05",
                dict(
                    status="ok",
                    result=dict(
                        results=[dict(slug="concepts/a")],
                        fallback=None,
                        coverage=dict(total=2, embedded=1, pending=1, rejected=0),
                    ),
                ),
            )

    def test_scale_query_must_return_the_exact_twenty_one_nodes_and_110_edges(self):
        report = scale_fixture(10_000)
        driver.scale_graph_contract(report, 10_000)
        with self.assertRaises(RuntimeError):
            driver.scale_graph_contract(dict(report, nodes=[], relations=[]), 10_000)
        report["relations"][0]["source"] = "concepts/foreign"
        with self.assertRaises(RuntimeError):
            driver.scale_graph_contract(report, 10_000)

    def test_tuning_set_is_disjoint_and_reports_both_cutoffs(self):
        cases = tuning_cases()
        self.assertEqual(
            {c["name"] for c in cases},
            {
                "one-edge",
                "chain",
                "star",
                "diamond",
                "cycle",
                "mixed",
                "duplicates",
                "disconnected",
            },
        )
        self.assertTrue(all(c["prefix"].startswith("tuning/") and c["relevant"] for c in cases))
        duplicate = next(c for c in cases if c["name"] == "duplicates")
        self.assertEqual((len(duplicate["authored"]), len(duplicate["expected"])), (2, 1))
        disconnected = next(c for c in cases if c["name"] == "disconnected")
        self.assertEqual(disconnected["relevant"], {"tuning/disconnected/b"})
        self.assertEqual(rank_metrics(["a", "x", "y", "b"], {"a", "b"}, 3)["recall"], 0.5)
        self.assertEqual(
            rank_metrics(["a", "x", "y", "b"], {"a", "b"}, 10),
            dict(cutoff=10, relevant_retrieved=2, relevant_total=2, precision=0.2, recall=1.0),
        )

    def test_paired_evidence_is_retained_before_comparison_or_transport_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            peer = object.__new__(driver.Peer)
            peer.root, peer.socket = Path(directory) / "knowledge", Path(directory) / "unused.sock"
            peer.sequence, peer.credential = 0, "fixture-private-token"
            operation = dict(op="search", mode="text", request=dict(query="needle"))
            reply = dict(status="ok", result=dict(results=[]))
            with patch("release.compatibility_transcripts.exchange", return_value=reply):
                self.assertEqual(peer.call(operation), reply)
            with self.assertRaises(RuntimeError):
                driver.ordinary_control("O01", reply)
            with patch(
                "release.compatibility_transcripts.exchange", side_effect=OSError("transport")
            ):
                with self.assertRaises(OSError):
                    peer.call(operation)
            raw = (Path(directory) / "requests.ndjson").read_text()
            rows = [json.loads(line) for line in raw.splitlines()]
            self.assertNotIn(peer.credential, raw)
            self.assertEqual(len(rows), 3)
            self.assertEqual(rows[0]["operation"], operation)
            self.assertEqual(rows[1]["reply"], reply)
            self.assertGreaterEqual(rows[1]["elapsed_ms"], 0)
            self.assertEqual(rows[2]["sequence"], 2)

    def test_case_ids_and_modes_are_distinct(self):
        cases = dict(ordinary_cases())
        self.assertEqual(len(cases), 15)
        self.assertEqual(set(cases), {f"O{i:02}" for i in range(1, 14)} | {"O16", "O17"})
        self.assertEqual(cases["O05"]["mode"], "vector")
        self.assertEqual(cases["O06"]["mode"], "hybrid")
        self.assertEqual(cases["O13"]["request"]["limit"], 1)
        for name, operation in cases.items():
            if name != "O17":
                self.assertNotIn("graph", operation["request"])
                self.assertIn("before", operation["request"])  # Disable wall-clock recency.

    def test_percentiles_preserve_raw_observations(self):
        values = [10, 1, 2, 9, 3, 8, 4, 7, 5, 6]
        result = timings(values)
        self.assertEqual(result["samples_ms"], values)
        self.assertEqual(result["p50_ms"], 5.5)
        self.assertAlmostEqual(result["p95_ms"], 9.55)
        self.assertAlmostEqual(result["p99_ms"], 9.91)
        self.assertEqual(percentile([7], 0.95), 7)
