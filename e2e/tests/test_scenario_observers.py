"""A slow control reply must not blind the committed-progress observer."""

import io
import json
import sqlite3
import sys
import tempfile
import threading
import time
import unittest
from contextlib import closing, redirect_stdout
from pathlib import Path
from unittest.mock import patch

from e2e.scripts.run_mock_ping_pong_pair import parse_args, run
from e2e.scripts.scenario_observers import Observers
from release.files import atomic_write_text


class ObserverTests(unittest.TestCase):
    def setUp(self):
        from release.cancellation import scope

        self.scope = scope()
        self.scope.__enter__()
        self.addCleanup(self.scope.__exit__, None, None, None)

    def test_progress_keeps_sampling_and_stop_joins_blocked_control(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "runtime").mkdir()
            with closing(sqlite3.connect(root / "runtime/runtime.sqlite")) as db, db:
                db.executescript(
                    "PRAGMA journal_mode=WAL; CREATE TABLE events(seq INTEGER PRIMARY KEY); CREATE TABLE runtime_counts(id INTEGER,active INTEGER); INSERT INTO runtime_counts VALUES(1,2);"
                )
            entered, release = threading.Event(), threading.Event()

            def blocked_ping(*_args):
                entered.set()
                release.wait(5)
                return {}, dict(seconds=0)

            observer = Observers(root, root, 1, 0.05)
            stopper = None
            with patch("e2e.scripts.scenario_observers.ping", blocked_ping):
                try:
                    observer.start()
                    self.assertTrue(entered.wait(2))
                    path = root / "fast-progress.jsonl"
                    deadline = time.monotonic() + 2
                    while time.monotonic() < deadline:
                        if path.exists() and len(path.read_text().splitlines()) >= 3:
                            break
                        time.sleep(0.01)
                    observations = [json.loads(line) for line in path.read_text().splitlines()]
                    self.assertGreaterEqual(len(observations), 3)
                    for observation in observations:
                        self.assertGreater(observation["database_bytes"], 0)
                        self.assertLessEqual(
                            observation["after_monotonic_ns"],
                            observation["storage_before_monotonic_ns"],
                        )
                        self.assertLessEqual(
                            observation["storage_before_monotonic_ns"],
                            observation["storage_after_monotonic_ns"],
                        )
                    stopper = threading.Thread(target=observer.stop)
                    stopper.start()
                    stopper.join(0.05)
                    self.assertTrue(stopper.is_alive())
                finally:
                    release.set()
                    if stopper:
                        stopper.join(2)
                    observer.stop()
            self.assertFalse(any(t.is_alive() for t in observer.threads))
            report = json.loads((root / "observer-accounting.json").read_text())
            self.assertEqual(report["errors"], [])
            self.assertGreaterEqual(report["threads"]["progress"]["samples"], 3)
            self.assertGreater(report["threads"]["progress"]["cpu_ns"], 0)

    def test_observer_failures_preserve_workload_audit_and_daemon_cleanup(self):
        original_write = atomic_write_text

        def fail_accounting(path, *args, **kwargs):
            if path.name == "observer-accounting.json":
                raise OSError("injected accounting failure")
            return original_write(path, *args, **kwargs)

        for kind in ("probe", "accounting"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / "run"
                args = parse_args(
                    [
                        "--agents",
                        "100",
                        "--observer-interval",
                        "0.05",
                        "--output-dir",
                        str(output),
                        "--timeout",
                        "120",
                    ]
                )
                failure = (
                    patch(
                        "e2e.scripts.scenario_observers.ping",
                        return_value=(None, dict(error="injected probe failure")),
                    )
                    if kind == "probe"
                    else patch("e2e.scripts.scenario_observers.atomic_write_text", fail_accounting)
                )
                with failure, redirect_stdout(io.StringIO()):
                    with self.assertRaisesRegex(
                        RuntimeError, "Pair audit passed, but control comparison failed"
                    ):
                        run(args)
                result = json.loads((output / "verification.json").read_text())
                self.assertEqual(result["result"], "failed")
                self.assertEqual(result["agents"], 100)
                self.assertIn("injected", result["control_error"])
                self.assertGreater(result["confirmed_provider_shutdowns"], 0)
                self.assertFalse((output / "cleanup-error.json").exists())
                retained = Path((output / "failure-state.txt").read_text().strip())
                self.assertEqual(retained, (output / "state").resolve())
                self.assertTrue((retained / "runtime/runtime.sqlite").is_file())

    def test_invalid_cadence_rejected(self):
        for value in (-1, 0, 0.001, float("nan"), float("inf"), 11):
            with self.assertRaises(ValueError):
                Observers(".", ".", 1, value)


if __name__ == "__main__":
    unittest.main()
