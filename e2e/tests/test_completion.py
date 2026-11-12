"""Final success belongs after resource cleanup and the cancellation boundary."""

from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import signal
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from e2e.scripts import run_mock_ping_pong_pair as pairs
from e2e.scripts import run_mock_ping_pong_all_to_one as all_to_one
from release.progress import entry
from release.cancellation import finish, scope


class CompletionTests(unittest.TestCase):
    def scenario(self, module, root, cleanup):
        kind = pairs.PairRun if module is pairs else all_to_one.AllToOneRun
        scenario = object.__new__(kind)
        names = (
            "start",
            "submit",
            "wait_for_completion",
            "shutdown",
            "verify_control",
            "verify_evidence",
            "collect_timings",
            "measure",
            "retain_evidence",
            "verify",
            "prepare_report",
        )
        for name in names:
            setattr(scenario, name, lambda: None)
        scenario.cleanup = cleanup
        scenario.report = lambda: (root / "verification.json").write_text('"passed"')
        return scenario

    def test_cleanup_failure_never_publishes_success(self):
        for module in (pairs, all_to_one):
            with self.subTest(module=module.__name__), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)

                def cleanup():
                    self.assertFalse((root / "verification.json").exists())
                    raise OSError("injected final owner failure")

                scenario = self.scenario(module, root, cleanup)
                with patch.object(pairs, "PairMonitor"), self.assertRaises(OSError):
                    result = scenario.run()
                    finish(result.report)
                self.assertFalse((root / "verification.json").exists())

    def reporting_scenario(self, module, root):
        kind = pairs.PairRun if module is pairs else all_to_one.AllToOneRun
        scenario = object.__new__(kind)
        scenario.output = root
        scenario.args = SimpleNamespace(
            preset="high-residency",
            binary=root / "release/hivectl",
            process_variants=1,
            logins=1,
            deltas=False,
            root_rate=0,
            debug_timings=False,
        )
        scenario.result = dict(messages=4, pairs=1)
        scenario.success = False
        scenario.limits = {}
        if module is all_to_one:
            scenario.limits["max_providers"] = 4
        scenario.temporary = root / "temporary"
        scenario.state = scenario.temporary / "state"
        (scenario.state / "runtime/output").mkdir(parents=True)
        (root / "failure-state.txt").write_text(str(scenario.state))
        if module is pairs:
            scenario.control_error = scenario.timings_error = None
            scenario.timings = dict(scenario_seconds=1)
            scenario.monitor = SimpleNamespace(backlog_scan_seconds=0)
            scenario.host = SimpleNamespace(seconds=0)
            scenario.trace = dict(first_start=1, last_turn=1000000001)
            scenario.peak_total = scenario.peak_daemon = scenario.peak_self = 0
            scenario.samples, scenario.sizes = 1, {}
        else:
            scenario.elapsed = 1
            (root / "memory.jsonl").write_text(
                json.dumps(dict(total_rss_kib=1, daemon_rss_kib=1)) + "\n"
            )
            (root / "runtime.sqlite").touch()
        return scenario

    def test_actual_pair_report_unlink_failure_cannot_leave_passed_verification(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            scenario = self.reporting_scenario(pairs, root)
            unlink = Path.unlink

            def fail(path, *args, **kwargs):
                if path.name == "failure-state.txt":
                    raise OSError("injected unlink failure")
                return unlink(path, *args, **kwargs)

            with patch.object(Path, "unlink", fail), self.assertRaises(OSError):
                scenario.prepare_report()
                finish(scenario.report)
            self.assertFalse((root / "verification.json").exists())
            self.assertFalse(scenario.success)

    def test_signal_during_actual_all_to_one_temporary_cleanup_prevents_success(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            scenario = self.reporting_scenario(all_to_one, root)
            rmtree = all_to_one.shutil.rmtree

            def work():
                with scope() as state:

                    def cleanup(path, *args, **kwargs):
                        rmtree(path, *args, **kwargs)
                        state.record(signal.SIGTERM, None)

                    with patch.object(all_to_one.shutil, "rmtree", cleanup):
                        scenario.prepare_report()
                    finish(scenario.report)

            with redirect_stdout(io.StringIO()) as output:
                self.assertEqual(entry(work, name="scenario"), 128 + signal.SIGTERM)
            self.assertEqual(output.getvalue(), "")
            self.assertFalse((root / "verification.json").exists())
            self.assertFalse(scenario.success)

    def test_signal_after_cleanup_before_completion_never_publishes_success(self):
        for module in (pairs, all_to_one):
            with self.subTest(module=module.__name__), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)

                def work():
                    with scope() as state:
                        scenario = self.scenario(
                            module, root, lambda: state.record(signal.SIGTERM, None)
                        )
                        with patch.object(pairs, "PairMonitor"):
                            result = scenario.run()
                        finish(result.report)

                self.assertEqual(entry(work, name="scenario"), 128 + signal.SIGTERM)
                self.assertFalse((root / "verification.json").exists())
