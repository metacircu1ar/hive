"""Paced arrivals retain every identity and expose lateness independently."""

from e2e.scripts.harness.scenario import run as run_process

import io
import json
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[1]
from e2e.scripts.run_mock_ping_pong_pair import parse_args
from e2e.scripts.scenario_arrivals import submit
from release.files import AppendStream
from e2e.scripts.harness.io import json_lines


class ArrivalTests(unittest.TestCase):
    def test_delayed_acknowledgement_catches_up_without_moving_deadlines(self):
        now = 0

        def sleep(seconds):
            nonlocal now
            now += round(seconds * 1e9)

        def slow_ack():
            nonlocal now
            now += 250_000_000
            return [{}]

        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        path = Path(temporary.name) / "arrivals"
        stream = AppendStream.create_new(path, durable=False).__enter__()
        self.addCleanup(stream.close)
        check, monitor = Mock(), Mock()
        with (
            patch("e2e.scripts.scenario_arrivals.time.monotonic_ns", side_effect=lambda: now),
            patch("e2e.scripts.scenario_arrivals.pause", side_effect=sleep),
        ):
            submit(stream, 0, 10, 0, 1, slow_ack, check, monitor)
            submit(stream, 0, 10, 1, 1, lambda: [{}], check, monitor)
            submit(stream, 0, 10, 2, 1, lambda: [{}], check, monitor)
            submit(stream, 0, 10, 3, 1, lambda: [{}], check, monitor)
        rows = list(json_lines(path))
        self.assertEqual(
            [r["scheduled_monotonic_ns"] for r in rows], [0, 100_000_000, 200_000_000, 300_000_000]
        )
        self.assertEqual(
            [r["started_monotonic_ns"] for r in rows], [0, 250_000_000, 250_000_000, 300_000_000]
        )
        monitor.assert_not_called()

    def test_expired_catch_up_is_not_submitted_and_incomplete_ack_is_retained(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        path = Path(temporary.name) / "arrivals"
        stream = AppendStream.create_new(path, durable=False).__enter__()
        self.addCleanup(stream.close)
        call = Mock()
        with patch("e2e.scripts.scenario_arrivals.time.monotonic_ns", return_value=2_000_000_000):
            with self.assertRaisesRegex(RuntimeError, "deadline"):
                submit(
                    stream, 0, 10, 1, 1, call, Mock(side_effect=RuntimeError("deadline")), Mock()
                )
            call.assert_not_called()
            with self.assertRaisesRegex(RuntimeError, "Incomplete seed batch"):
                submit(stream, 0, 10, 1, 1, lambda: [], Mock(), Mock())
        (row,) = json_lines(path)
        self.assertFalse(row["acknowledged"])
        self.assertEqual(row["started_monotonic_ns"], row["finished_monotonic_ns"])

    def test_invalid_rates_rejected(self):
        for rate in ("nan", "inf", "-1", "0.001", "1001"):
            with (
                self.subTest(rate=rate),
                redirect_stderr(io.StringIO()),
                self.assertRaises(SystemExit),
            ):
                parse_args(["--root-rate", rate])
        self.assertEqual(parse_args([]).root_rate, 0)

    def test_paced_pairs_have_fixed_deadlines_and_complete(self):
        with tempfile.TemporaryDirectory(prefix="hl-pair-arrivals-") as directory:
            output = Path(directory) / "run"
            run = run_process(
                [
                    sys.executable,
                    str(ROOT / "run.py"),
                    "mock",
                    "pairs",
                    "--agents",
                    "100",
                    "--root-rate",
                    "10",
                    "--output-dir",
                    str(output),
                    "--timeout",
                    "120",
                ],
                capture_output=True,
                text=True,
                timeout=180,
            )
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
            result = json.loads((output / "verification.json").read_text())
            self.assertEqual(
                (result["result"], result["root_rate"], result["pairs"]), ("passed", 10, 50)
            )
            rows = [
                json.loads(line)
                for line in (output / "arrival-schedule.jsonl").read_text().splitlines()
            ]
            self.assertEqual([row["offset"] for row in rows], list(range(50)))
            epoch = rows[0]["scheduled_monotonic_ns"]
            for i, row in enumerate(rows):
                self.assertEqual(row["scheduled_monotonic_ns"], epoch + i * 100_000_000)
                self.assertEqual(row["count"], 1)
                self.assertTrue(row["acknowledged"])
                self.assertGreaterEqual(row["started_monotonic_ns"], row["scheduled_monotonic_ns"])
                self.assertGreaterEqual(row["finished_monotonic_ns"], row["started_monotonic_ns"])
                if i:
                    self.assertGreaterEqual(
                        row["started_monotonic_ns"], rows[i - 1]["finished_monotonic_ns"]
                    )
