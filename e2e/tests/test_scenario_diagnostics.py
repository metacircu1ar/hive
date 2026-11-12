"""Failure injection into the real harness without provider processes or a live daemon."""

import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from e2e.scripts.harness.cli import Cli
from e2e.scripts.run_mock_ping_pong_pair import PairRun, PairMonitor
from e2e.scripts.scenario_memory import process_table, host_snapshot


class DiagnosticTests(unittest.TestCase):
    def test_host_command_failure_is_explicitly_incomplete(self):
        with patch("e2e.scripts.scenario_memory.run_process", side_effect=OSError("unavailable")):
            captured = host_snapshot()
        self.assertTrue(captured)
        self.assertTrue(
            all(row["complete"] is False and "error" in row for row in captured.values())
        )

    def test_partial_process_table_is_not_successful_rss_evidence(self):
        result = subprocess.CompletedProcess([], 0, "123 12 missing-columns\n", "")
        with patch("e2e.scripts.scenario_memory.run_process", return_value=result):
            with self.assertRaisesRegex(RuntimeError, "RSS observation unavailable"):
                process_table()

    def test_pair_cleanup_continues_after_observer_failure_and_retains_evidence(self):
        run = PairRun.__new__(PairRun)
        original = OSError("observer failed")
        seen = []

        def observer():
            seen.append("observer")
            raise original

        run.stop_observers = observer
        run.stop_sampler = lambda: seen.append("sampler")
        run.stop_daemon = lambda: seen.append("daemon")
        run.close_database = lambda: seen.append("database")
        run.retain_failure = lambda: seen.append(("retained", run.success))
        run.success = True
        with self.assertRaises(OSError) as raised:
            run.cleanup()
        self.assertIs(raised.exception, original)
        self.assertEqual(seen, ["observer", "sampler", "daemon", "database", ("retained", False)])

    def test_snapshot_failure_survives_failed_rollback(self):
        original = OSError("query failed")
        database = Mock()
        database.execute.side_effect = [None, original]
        database.rollback.side_effect = RuntimeError("rollback failed")
        run = SimpleNamespace(database=database, args=SimpleNamespace(timeout=5))
        with self.assertRaises(OSError) as raised:
            PairMonitor(run).snapshot(1)
        self.assertIs(raised.exception, original)
        database.rollback.assert_called_once_with()

    def test_status_failure_survives_failed_evidence_write(self):
        original = OSError("status failed")
        run = SimpleNamespace(
            call=Mock(side_effect=original), output=Path("/unused"), args=SimpleNamespace(timeout=5)
        )
        with patch(
            "e2e.scripts.run_mock_ping_pong_pair.append_jsonl_record",
            side_effect=RuntimeError("write failed"),
        ):
            with self.assertRaises(OSError) as raised:
                PairMonitor(run).control_status(1)
        self.assertIs(raised.exception, original)


if __name__ == "__main__":
    unittest.main()
