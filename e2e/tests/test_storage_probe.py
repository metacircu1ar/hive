"""A comparison observer cannot leave a thread blocked in a native disk flush."""

import os
from pathlib import Path
import signal
import tempfile
import unittest

from e2e.scripts.run_arms import Probe
from release.cancellation import pause, scope
from release.process import ProcessFailure


class ProbeTests(unittest.TestCase):
    def test_process_probe_stops_and_retains_complete_samples(self):
        with tempfile.TemporaryDirectory() as temp, scope():
            with Probe(Path(temp) / "probe", 0.05) as probe:
                pid = probe.child.pid
                pause(0.2)
            self.assertTrue(probe.samples)
            self.assertIsNotNone(probe.metrics())
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)

    def test_unexpected_probe_death_fails_with_actual_exit_status(self):
        with tempfile.TemporaryDirectory() as temp, scope():
            with self.assertRaises(ProcessFailure) as raised:
                with Probe(Path(temp) / "probe", 1) as probe:
                    os.kill(probe.child.pid, signal.SIGKILL)
                    pause(0.05)
            self.assertEqual(raised.exception.result.returncode, -signal.SIGKILL)
