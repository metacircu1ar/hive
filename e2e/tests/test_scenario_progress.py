"""Progress bounds and control-window selection must retain slow evidence."""

import json
import sys
import tempfile
import unittest
from pathlib import Path

from e2e.scripts.scenario_progress import summarize


class ProgressTests(unittest.TestCase):
    def test_frozen_windows_detect_pause_and_keep_boundary_reply(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            rows = []
            seq = 0
            for i in range(301):
                if not 100 <= i <= 140:
                    seq += 100
                rows.append(
                    dict(
                        probe_index=i,
                        before_monotonic_ns=i * 100_000_000,
                        after_monotonic_ns=i * 100_000_000 + 1000,
                        storage_before_monotonic_ns=i * 100_000_000 + 1100,
                        storage_after_monotonic_ns=i * 100_000_000 + 2100,
                        database_bytes=4096,
                        wal_bytes=None if i == 0 else 12345,
                        active=2,
                        event_seq=seq,
                    )
                )
            (root / "progress").write_text("".join(json.dumps(r) + "\n" for r in rows))
            controls = [
                dict(start_monotonic_ns=29_000_000_000, end_monotonic_ns=35_000_000_000),
                dict(start_monotonic_ns=31_000_000_000, end_monotonic_ns=31_001_000_000),
            ]
            (root / "control").write_text("".join(json.dumps(r) + "\n" for r in controls))
            result = summarize(root / "progress", root / "control")
            self.assertEqual(result["active_control_seconds"]["count"], 1)
            self.assertEqual(result["active_control_seconds"]["max"], 6)
            self.assertGreater(len(result["flagged_windows"]), 0)
            self.assertEqual(len(result["zero_progress_intervals"]), 1)
            self.assertEqual(result["observation_gaps"], [])
            self.assertEqual(result["storage_observations"]["count"], 301)
            self.assertEqual(result["storage_observations"]["missing_wal"], 1)
            for window in result["flagged_windows"]:
                self.assertLessEqual(window["lower_rate"], window["upper_rate"])
            rows[20]["after_monotonic_ns"] += 60_000_000
            rows[20]["storage_before_monotonic_ns"] += 60_000_000
            rows[20]["storage_after_monotonic_ns"] += 60_000_000
            (root / "progress").write_text("".join(json.dumps(r) + "\n" for r in rows))
            self.assertEqual(
                len(summarize(root / "progress", root / "control")["wide_query_brackets"]), 1
            )
            # A 1.3s slowdown at25% rate straddles a non-overlapping boundary.
            seq = 0
            for i, row in enumerate(rows):
                seq += 25 if 105 <= i <= 118 else 100
                row["event_seq"] = seq
            (root / "progress").write_text("".join(json.dumps(r) + "\n" for r in rows))
            short = summarize(root / "progress", root / "control")
            self.assertEqual(short["zero_progress_intervals"], [])
            self.assertTrue(short["flagged_episodes"])
            self.assertGreater(short["eligible_windows"], short["median_windows"])
            for name, value, expected in [
                ("database_bytes", 0, "Invalid storage observation"),
                ("database_bytes", True, "Invalid storage observation"),
                ("wal_bytes", -1, "Invalid storage observation"),
                ("storage_before_monotonic_ns", 0, "Reversed storage bracket"),
                ("storage_after_monotonic_ns", 10**15, "Overlapping storage observations"),
            ]:
                saved = rows[20][name]
                rows[20][name] = value
                (root / "progress").write_text("".join(json.dumps(r) + "\n" for r in rows))
                with (
                    self.subTest(field=name, value=value),
                    self.assertRaisesRegex(RuntimeError, expected),
                ):
                    summarize(root / "progress", root / "control")
                rows[20][name] = saved
            del rows[20]["wal_bytes"]
            (root / "progress").write_text("".join(json.dumps(r) + "\n" for r in rows))
            with self.assertRaisesRegex(RuntimeError, "Invalid storage observation"):
                summarize(root / "progress", root / "control")
            rows[20]["wal_bytes"] = 12345
            rows[20]["probe_index"] = 21
            (root / "progress").write_text("".join(json.dumps(r) + "\n" for r in rows))
            with self.assertRaisesRegex(RuntimeError, "Missing progress observation"):
                summarize(root / "progress", root / "control")


if __name__ == "__main__":
    unittest.main()
