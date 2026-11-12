"""Failures must not become successful probe or workload evidence."""

import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from contextlib import redirect_stdout

from e2e.scripts.run_mock_ping_pong_pair import PairRun


class AuditErrors(unittest.TestCase):
    def test_pair_report_never_publishes_success_for_auxiliary_failures(self):
        for failure in ("control_error", "timings_error", None):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                run = PairRun.__new__(PairRun)
                run.args = SimpleNamespace(
                    preset="high-residency",
                    root_rate=0,
                    debug_timings=True,
                    process_variants=1,
                    logins=1,
                    auth_mode="balanced",
                    deltas=0,
                )
                run.control_error = run.timings_error = None
                if failure:
                    setattr(run, failure, "injected failure")
                run.result = dict(messages=4, pairs=2)
                run.output = Path(directory)
                run.limits = run.sizes = {}
                run.timings = dict(scenario_seconds=1)
                run.monitor = SimpleNamespace(backlog_scan_seconds=0)
                run.host = SimpleNamespace(seconds=0)
                run.trace = dict(last_turn=2, first_start=1)
                run.peak_total = run.peak_daemon = run.peak_self = run.samples = 0
                run.success = False
                stdout = io.StringIO()
                with redirect_stdout(stdout):
                    if failure:
                        with self.assertRaisesRegex(RuntimeError, "comparison failed"):
                            run.prepare_report()
                            run.report()
                    else:
                        run.prepare_report()
                        run.report()
                verdict = json.loads((run.output / "verification.json").read_text())
                self.assertEqual(verdict["result"], "failed" if failure else "passed")
                self.assertEqual(run.success, failure is None)
                if failure:
                    self.assertEqual(stdout.getvalue(), "")
                else:
                    self.assertEqual(json.loads(stdout.getvalue())["result"], "passed")

    def test_cleanup_points_to_state_already_relocated_before_report_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            run = PairRun.__new__(PairRun)
            run.output = root / "output"
            run.output.mkdir()
            run.temporary = root / "temporary"
            run.temporary.mkdir()
            run.state = run.temporary / "state"
            run.state.mkdir()
            (run.state / "original-evidence").write_text("kept")
            (run.output / "audit.sqlite").write_text("derived")
            run.retained_state = None
            run.timings = {}
            run.began = 0
            run.harness_cpu = None
            run.retain_evidence()
            run.success = False
            run.observers = run.sampler = run.daemon = run.database = None
            run.cleanup()
            retained = Path((run.output / "failure-state.txt").read_text().strip())
            self.assertEqual(retained, run.output / "state")
            self.assertEqual((retained / "original-evidence").read_text(), "kept")
            self.assertFalse(run.temporary.exists())


if __name__ == "__main__":
    unittest.main()
