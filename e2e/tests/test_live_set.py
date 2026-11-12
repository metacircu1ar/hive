"""The default live set is serial, bounded and never hides unfinished checks."""

import contextlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from e2e.live import runner
from release.files import atomic_write_json, create_directory
from release.progress import entry


class LiveSetTests(unittest.TestCase):
    def test_serial_order_and_failure_retains_the_unfinished_tail(self):
        for failed in (None, "controls"):
            with self.subTest(failed=failed), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                output = root / "out"
                seen = []

                def child(command, **options):
                    name = command[command.index("live") + 1]
                    seen.append(name)
                    self.assertEqual(options["stop_grace"], 180)
                    self.assertEqual(
                        options["timeout"], int(command[command.index("--timeout") + 1]) + 180
                    )
                    self.assertIn("--live", command)
                    self.assertLessEqual(
                        int(command[command.index("--timeout") + 1]),
                        1800 if name == "shop-team" else 600,
                    )
                    if name == failed:
                        raise RuntimeError("Fixture child failure")
                    destination = Path(command[command.index("--output-dir") + 1])
                    create_directory(destination, mode=0o700)
                    row = next(row for row in runner.CHECKS if row[0] == name)
                    atomic_write_json(destination / row[2], {row[3]: "passed"})
                    return subprocess.CompletedProcess(command, 0, b"", b"")

                with (
                    patch.object(sys, "argv", ["live", "--live", "--output-dir", str(output)]),
                    patch("e2e.live.scenario.require_paths"),
                    patch.object(runner, "run", side_effect=child),
                    contextlib.redirect_stdout(io.StringIO()),
                ):
                    self.assertEqual(
                        entry(runner.main, name="scenario"), 0 if failed is None else 1
                    )
                self.assertEqual(seen, [r[0] for r in runner.CHECKS][: len(seen)])
                self.assertEqual((output / "result.json").exists(), failed is None)
                if failed is not None:
                    report = json.loads((output / "retained.json").read_text())
                    self.assertEqual(report["unfinished"], ["controls", "comb", "comb-graph"])

    def test_real_child_can_finish_work_in_cap_then_publish_after_cleanup(self):
        from release.process import run as native_run

        code = (
            "import e2e,time,sys; from pathlib import Path; "
            "from release.files import create_directory,atomic_write_json; "
            "time.sleep(.9); time.sleep(1.3); destination=Path(sys.argv[1]); "
            "create_directory(destination,mode=448); "
            "atomic_write_json(destination/'result.json',{'result':'passed'})"
        )
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "out"

            def child(command, **options):
                destination = command[command.index("--output-dir") + 1]
                return native_run([sys.executable, "-c", code, destination], **options)

            with (
                patch.object(
                    sys, "argv", ["live", "--live", "--timeout", "2", "--output-dir", str(output)]
                ),
                patch("e2e.live.scenario.require_paths"),
                patch.object(runner, "CHECKS", (("controls", 1, "result.json", "result"),)),
                patch.object(runner, "run", side_effect=child),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                self.assertEqual(entry(runner.main, name="scenario"), 0)
            self.assertEqual(json.loads((output / "result.json").read_text())["result"], "passed")

    def test_missing_opt_in_refuses_before_any_child_or_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "out"
            with (
                patch.object(sys, "argv", ["live", "--output-dir", str(output)]),
                patch.object(runner, "run") as child,
            ):
                self.assertEqual(entry(runner.main, name="scenario"), 2)
            child.assert_not_called()
            self.assertFalse(output.exists())
