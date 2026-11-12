"""Comparison provenance, native status and nested ownership regressions."""

import argparse
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from e2e.paths import ROOT
from e2e.scripts import run_arms
from release.cancellation import pause, scope
from release.owned_process import spawn


class ComparisonTests(unittest.TestCase):
    def arguments(self, binary, output, driver=()):
        return run_arms.parse_args(
            [
                "--arm",
                f"a={binary}",
                "--arm",
                f"b={binary}",
                "--pairs",
                "1",
                "--max-pairs",
                "1",
                "--cooldown",
                "0",
                "--output-dir",
                str(output),
                "--",
                *driver,
            ]
        )

    def test_forwarded_owned_options_equal_and_abbreviations_are_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            release = Path(temp) / "release"
            release.mkdir()
            binary = release / "hivectl"
            binary.touch()
            for option in (
                "--binary",
                "--binary=x",
                "--bin",
                "--b=x",
                "--output-dir",
                "--output-dir=x",
                "--output-d=x",
                "--output=x",
            ):
                with self.subTest(option=option), contextlib.redirect_stderr(io.StringIO()):
                    with self.assertRaises(SystemExit):
                        self.arguments(binary, Path(temp) / "out", [option])

    def test_session_hashes_in_chunks_without_file_digest(self):
        with tempfile.TemporaryDirectory() as temp:
            release = Path(temp) / "release"
            release.mkdir()
            binary = release / "hivectl"
            binary.write_bytes(b"synthetic binary" * 100000)
            expected = hashlib.sha256(binary.read_bytes()).hexdigest()
            with patch.object(hashlib, "file_digest", None, create=True):
                session = run_arms.Session(self.arguments(binary, Path(temp) / "out"))
            self.assertEqual(session.binaries, dict(a=expected, b=expected))

    def test_nonzero_child_cannot_make_a_passed_comparison(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp)
            args = argparse.Namespace(
                output_dir=output,
                arms=[],
                cooldown=0,
                barrier_delay_ms=None,
                probe_interval=None,
                driver=[],
                canary_floor_mibs=None,
            )
            session = run_arms.Session(args)
            session.binaries["a"] = "synthetic"
            directory = output / "pair00-0-a"
            directory.mkdir()
            (directory / "verification.json").write_text(json.dumps(dict(result="passed")))
            with (
                patch.object(run_arms, "canary", return_value=dict(full_sync_mibs=1)),
                patch.object(
                    run_arms, "run_scenario", return_value=subprocess.CompletedProcess([], 7)
                ),
                patch.object(session, "save"),
            ):
                row = session.run_arm(0, 0, "a", output / "release/hivectl")
            self.assertEqual(row["result"], "failed")
            self.assertEqual(row["returncode"], 7)
            session.runs.append(dict(row, slot=1, result="passed", returncode=0))
            self.assertEqual(session.same_mode_pairs(), [])

    def test_nested_runner_cancellation_allows_delayed_owned_daemon_cleanup(self):
        # The innermost owner has a different session; killing only the runner
        # cannot clean it. Its deliberately slow finalizer exceeds the default grace.
        script = """
import os, sys, time
from pathlib import Path
from release.progress import entry, finalizer
from release.owned_process import spawn
from release.cancellation import pause
from e2e.scripts.harness.scenario import run

def work():
    if sys.argv[1] == "outer":
        run([sys.executable, __file__, "inner", sys.argv[2]], timeout=30)
    else:
        with spawn([sys.executable, "-c", "import time; time.sleep(60)"]) as child:
            Path(sys.argv[2]).write_text(str(child.pid))
            with finalizer(lambda: time.sleep(0.8)):
                while True:
                    pause(0.02)
raise SystemExit(entry(work, name="scenario"))
"""
        with tempfile.TemporaryDirectory() as temp, scope():
            path, ready = Path(temp) / "nested.py", Path(temp) / "ready"
            path.write_text(script)
            environment = dict(
                os.environ, PYTHONPATH=str(ROOT) + os.pathsep + str(ROOT / "scripts")
            )
            with spawn(
                [sys.executable, str(path), "outer", str(ready)],
                env=environment,
                stop_grace=run_arms.SCENARIO_STOP_GRACE,
            ) as outer:
                deadline = time.monotonic() + 10
                while not ready.exists():
                    if outer.exited() or time.monotonic() >= deadline:
                        self.fail("Nested owner did not become ready")
                    pause(0.02)
                daemon_pid = int(ready.read_text())
                os.kill(outer.pid, signal.SIGTERM)
                self.assertEqual(outer.wait(timeout=15), 128 + signal.SIGTERM)
            with self.assertRaises(ProcessLookupError):
                os.kill(daemon_pid, 0)
