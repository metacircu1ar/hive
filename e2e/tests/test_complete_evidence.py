"""Final current evidence consumers reject truncated records and partial UTF-8."""

import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from release.files import atomic_write_bytes
from e2e.scripts import scenario_control, scenario_memory
from e2e.scripts.scenario_progress import read_controls


class CompleteEvidenceTests(unittest.TestCase):
    def test_control_consumers_reject_truncated_slow_or_error_samples(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "control"
            row = dict(
                kind="cli", probe_index=0, start_monotonic_ns=0, end_monotonic_ns=1000, status={}
            )
            for tail in (b'{"error":"slow', b'{"start_monotonic_ns":10,"end_monotonic_ns":999999'):
                atomic_write_bytes(path, json.dumps(row).encode() + b"\n" + tail)
                with self.assertRaisesRegex(ValueError, "Incomplete evidence"):
                    read_controls(path, 0, 10000)
                with patch.object(scenario_control, "validate_status"):
                    with self.assertRaisesRegex(ValueError, "Incomplete evidence"):
                        scenario_control.summarize(path, 1)

    def test_memory_consumer_never_parses_partial_utf8(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "memory"
            sample = dict(processes=[], time=2, daemon_rss_kib=5, daemon={"pid": 1})
            atomic_write_bytes(path, json.dumps(sample).encode() + b'\n{"error":"\xe2')
            with patch.object(
                scenario_memory, "execution_owners", return_value=({}, [0], float("inf"))
            ):
                with self.assertRaisesRegex(ValueError, "Incomplete evidence"):
                    scenario_memory.summarize(path, [], {})
