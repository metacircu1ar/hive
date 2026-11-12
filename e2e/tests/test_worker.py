"""Actual child ownership and cancellation across sampler-style worker threads."""

import os
from pathlib import Path
import sys
import tempfile
import threading
import time
import unittest

from e2e.scripts.harness.worker import Worker
from release.cancellation import pause, scope
from release.native_evidence import evidence
from release.process import ProcessFailure, run


class WorkerTests(unittest.TestCase):
    def test_actual_worker_child_failure_retains_status_and_isolates_native_evidence(self):
        with (
            tempfile.TemporaryDirectory() as temp,
            scope(),
            evidence(Path(temp) / "native") as sink,
        ):
            worker = Worker(
                lambda: run(
                    [sys.executable, "-c", "import sys; sys.stderr.write('worker'); sys.exit(7)"],
                    check=True,
                ),
                stop=threading.Event(),
            )
            worker.start()
            with self.assertRaises(ProcessFailure) as raised:
                worker.join()
            self.assertEqual(raised.exception.result.returncode, 7)
            self.assertEqual(raised.exception.result.stderr, b"worker")
            self.assertEqual(sink.size, 0)
            self.assertFalse(worker.is_alive())

    def test_stop_during_actual_child_call_confirms_child_absence_before_join(self):
        with tempfile.TemporaryDirectory() as temp, scope():
            ready = Path(temp) / "ready"
            script = "import os,sys,time; from pathlib import Path; Path(sys.argv[1]).write_text(str(os.getpid())); time.sleep(60)"
            stop = threading.Event()
            worker = Worker(
                lambda: run([sys.executable, "-c", script, str(ready)], timeout=70), stop=stop
            )
            worker.start()
            try:
                deadline = time.monotonic() + 10
                while not ready.exists():
                    worker.check()
                    if time.monotonic() >= deadline:
                        self.fail("Worker child did not start")
                    pause(0.02)
                pid = int(ready.read_text())
            finally:
                stop.set()
                worker.join()
            self.assertFalse(worker.is_alive())
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)
