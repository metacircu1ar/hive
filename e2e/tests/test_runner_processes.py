"""Actual shell cancellation allows a nested Python owner to retire its child."""

import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import unittest

from e2e.tests.fixtures.campaign_service import request
from e2e.tests.test_campaign_processes import ROOT, SERVICE, process_absent, ready
from release.files import create_directory
from release.owned_process import spawn
from release.progress import finalizer

PARENT = """
import shlex,sys
from pathlib import Path
sys.path.insert(0,sys.argv[1])
import e2e
from e2e.ci import runner
from release.progress import entry
root=Path(sys.argv[2])
runner.STEP_SECONDS=1 if sys.argv[4]=='timeout' else 30
def main():
    owner=runner.Runner('macos',root,root/'output')
    command=shlex.join([sys.executable,sys.argv[3],'nested-owner',str(root)])
    if not owner.step('fixture','nested cleanup',command+' && printf UNEXPECTED_NEXT_COMMAND'):
        raise RuntimeError('Fixture step failed')
raise SystemExit(entry(main,name='scenario'))
"""


class RunnerProcessTests(unittest.TestCase):
    def test_cancel_and_timeout_wait_for_delayed_separate_session_cleanup(self):
        for mode, status in (("cancel", 143), ("timeout", 1)):
            with (
                self.subTest(mode=mode),
                tempfile.TemporaryDirectory(prefix="hive-step-", dir="/tmp") as temporary,
            ):
                root = Path(temporary)
                create_directory(root / "output")

                def retire():
                    if (root / "native-ready.json").exists() and not (
                        root / "native-done.json"
                    ).exists():
                        request("unix://" + str(root / "native.sock"), ["quit"])
                    if (root / "native-ready.json").exists():
                        process_absent(json.loads((root / "native-ready.json").read_text())["pid"])

                with (
                    finalizer(retire),
                    spawn(
                        [sys.executable, "-c", PARENT, str(ROOT), str(root), str(SERVICE), mode],
                        stop_grace=5,
                    ) as parent,
                ):
                    ready(root / "owner-ready.json", parent)
                    pid = json.loads((root / "native-ready.json").read_text())["pid"]
                    if mode == "cancel":
                        os.kill(parent.pid, signal.SIGTERM)
                    self.assertEqual(parent.wait(timeout=5), status)
                    self.assertTrue(
                        (root / "owner-done.json").exists(),
                        "Shell killed the cooperative owner before it finished cleanup",
                    )
                    process_absent(pid)
                    self.assertNotIn(
                        "UNEXPECTED_NEXT_COMMAND",
                        (root / "output/logs/fixture--nested-cleanup.log")
                        .read_text()
                        .split("\n", 1)[1],
                    )
