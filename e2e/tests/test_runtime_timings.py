"""Current DEBUG collection preserves bounded/partial evidence without old sidecars."""

import io
import json
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
from e2e.scripts.run_mock_ping_pong_pair import parse_args
from e2e.scripts.runtime_timings import collect


class TimingTests(unittest.TestCase):
    def test_info_default_and_retired_flags_rejected(self):
        self.assertFalse(parse_args([]).debug_timings)
        self.assertTrue(parse_args(["--debug-timings"]).debug_timings)
        for flag in ["--profile", "--diagnostics"]:
            with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                parse_args([flag])

    def test_rotations_loss_and_missing_start_remain_explicit(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            logs = root / "state/runtime/logs"
            logs.mkdir(parents=True)
            rows = [{"event": "writer.batch", "start_ns": 10, "end_ns": 20, "duration_ns": 9}]
            (logs / ".hive-daemon.jsonl.2.log").write_text(json.dumps(rows[0]) + "\n")
            rows += [
                {"event": "runtime.timing", "start_ns": 30, "end_ns": 40, "duration_ns": 9},
                {"event": "log.dropped", "full": 7},
                {"event": "daemon.stopped", "success": True},
            ]
            (logs / "hive-daemon.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows[1:]))
            result = collect(root / "state", root, 123)
            self.assertFalse(result["exhaustive"])
            self.assertFalse(result["observed_start"])
            self.assertTrue(result["observed_clean_stop"])
            self.assertEqual(result["loss_records"][0]["full"], 7)
            self.assertEqual(result["events"], {"writer.batch": 1, "runtime.timing": 1})
            self.assertEqual(
                [f["path"] for f in result["files"]],
                [".hive-daemon.jsonl.2.log", "hive-daemon.jsonl"],
            )

    def test_invalid_interval_and_truncated_tail_are_rejected(self):
        for text in [
            json.dumps({"event": "state.sync", "duration_ns": 1, "start_ns": 20, "end_ns": 10})
            + "\n",
            '{"event":',
        ]:
            with tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                logs = root / "state/runtime/logs"
                logs.mkdir(parents=True)
                (logs / "hive-daemon.jsonl").write_text(text)
                with self.assertRaises((ValueError, RuntimeError)):
                    collect(root / "state", root, 123)


if __name__ == "__main__":
    unittest.main()
