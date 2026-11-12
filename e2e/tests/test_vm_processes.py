"""Kill a real start client: its detached child is recovered only by owned inspection."""

from functools import partial
import json
import os
from pathlib import Path
import signal
import shutil
import sys
import tempfile
import unittest
from unittest.mock import patch

from e2e.campaign import Admission, read_receipt, recover
from e2e.tests.fixtures.campaign_service import request
from e2e.tests.test_campaign_processes import ready
from e2e.vm.images import config_bytes
from e2e.vm.lima import Lima, identity, owner_state
from e2e.vm.folder import delete as delete_vm
from release.cancellation import check, pause
from release.files import atomic_write_bytes, atomic_write_json, create_directory, record_lock
from release.owned_process import spawn
from release.process import run
from release.progress import finalizer

FIXTURE = Path(__file__).parent / "fixtures/vm_client.py"


def native(root, argv, **options):
    # Keep the real custodian and its descriptors; replace only its native CLI.
    boundary = argv.index("--")
    if argv[boundary + 1] != "limactl":
        raise ValueError("Unexpected fixture native executable")
    return run(
        [*argv[: boundary + 1], sys.executable, str(FIXTURE), str(root), *argv[boundary + 2 :]],
        **options,
    )


PARENT = """
import sys
from pathlib import Path
sys.path.insert(0,sys.argv[1])
import e2e
from e2e.campaign import Admission
from e2e.vm.lima import Lima
from e2e.tests.test_vm_processes import native
from functools import partial
from release.progress import entry
root=Path(sys.argv[2])
def main():
    admission=Admission(root/'admission')
    with admission.held():
        actor=Lima.load(admission,root/'out',native=partial(native,root))
        actor.begin('fixture-vm')
        actor.start()
raise SystemExit(entry(main,name='scenario'))
"""


class VmProcesses(unittest.TestCase):
    def test_detached_child_of_killed_client_has_no_lock_and_recovery_requires_stop(self):
        self.exercise(False)

    def test_killed_parent_retains_lock_only_until_local_native_client_is_gone(self):
        self.exercise(True)

    def test_killed_parent_and_custodian_leave_durable_fence_while_native_cli_lives(self):
        self.exercise(True, True)

    def exercise(self, kill_parent, kill_custodian=False):
        with (
            tempfile.TemporaryDirectory(prefix="hv-", dir="/tmp") as temporary,
            patch(
                "e2e.vm.folder.default_home", side_effect=lambda: Path(temporary).resolve() / "lima"
            ),
        ):
            root = Path(temporary).resolve()
            create_directory(root / "out", mode=0o700)
            admission = Admission(root / "admission")
            with admission.held():
                info = os.fstat(admission.lock_fd)
                atomic_write_json(root / "expected.json", [info.st_dev, info.st_ino])
                value = owner_state(
                    root / "out",
                    root / "lima",
                    nonce="a" * 32,
                    version="24.04",
                    memory_gib=8,
                    commit="b" * 40,
                    mode="full",
                    image=root / "image.img",
                )
                actor = Lima(admission, root / "out", value)
                directory = actor.home / actor.name
                create_directory(directory, mode=0o700)
                atomic_write_bytes(directory / "lima.yaml", config_bytes(value["config"]))
                actor.state.update(
                    create_completed=True,
                    create_settled=True,
                    directory_identity=identity(directory, directory=True),
                    config_identity=identity(directory / "lima.yaml"),
                )
                actor.save()
                atomic_write_json(
                    root / "row.json",
                    dict(
                        name=actor.name,
                        dir=str(directory),
                        status="Stopped",
                        vmType="vz",
                        memory=8 * 1024**3,
                        cpus=4,
                        config=value["config"],
                    ),
                )
            with spawn(
                [sys.executable, "-c", PARENT, str(Path(__file__).resolve().parents[2]), str(root)]
            ) as parent:
                try:
                    ready(root / "client.json", parent)
                except RuntimeError:
                    self.fail(bytes(parent.errors).decode(errors="replace"))
                saved = json.loads((root / "worker.json").read_text())
                current = json.loads((root / "client.json").read_text())
                pid = current["pid"]
                self.assertEqual(saved["parent"], pid)
                self.assertFalse(saved["inherited"])

                def retire_fixture():
                    if kill_custodian:
                        os.kill(pid, signal.SIGKILL)
                    if not (root / "worker-done.json").exists():
                        request("unix://" + str(root / "work.sock"), ["quit"])
                    ready(root / "worker-done.json")

                with finalizer(retire_fixture):
                    if kill_parent:
                        os.kill(parent.pid, signal.SIGKILL)
                        self.assertEqual(parent.wait(timeout=2), -signal.SIGKILL)
                        with (
                            self.assertRaises(TimeoutError),
                            Admission(admission.root).held(timeout=0.05),
                        ):
                            self.fail("Custodian must keep admission while its native CLI runs")
                    if kill_custodian:
                        os.kill(current["custodian"], signal.SIGKILL)
                        competitor = Admission(admission.root)
                        competitor.recover = lambda active: recover(
                            active, native=partial(native, root), admission=competitor
                        )
                        with self.assertRaises(RuntimeError), competitor.held(timeout=5):
                            self.fail("Missing durable completion must fence a living native CLI")
                        request("unix://" + str(root / "work.sock"), ["materialize", "Running"])
                        with self.assertRaises(RuntimeError), competitor.held(timeout=1):
                            self.fail("Remote Running cannot replace the missing local completion")
                        os.kill(pid, 0)
                        self.assertFalse((root / "worker-done.json").exists())
                        return
                    # Fresh PID from this live fixture boundary, never a product
                    # recovery strategy or permission to kill a saved process.
                    os.kill(pid, signal.SIGKILL)
                    if not kill_parent:
                        self.assertNotEqual(parent.wait(timeout=5), 0)
                    else:
                        shutil.rmtree(root / "out")
                    pending = read_receipt(admission.root)
                    self.assertEqual(pending["phase"], "starting")
                    competitor = Admission(admission.root)
                    competitor.recover = lambda active: recover(
                        active, native=partial(native, root), admission=competitor
                    )
                    with self.assertRaises(RuntimeError), competitor.held(timeout=5):
                        self.fail("Pre-PID Stopped cannot settle the interrupted start")
                    self.assertEqual(read_receipt(admission.root)["phase"], "starting")
                    self.assertFalse((root / "worker-done.json").exists())
                    if not kill_parent:
                        with record_lock(
                            admission.root / "admission", check=check, pause=pause, timeout=1
                        ) as fd:
                            competitor.lock_fd = fd
                            with finalizer(lambda: setattr(competitor, "lock_fd", None)):
                                competitor.active = read_receipt(admission.root)
                                with self.assertRaises(RuntimeError):
                                    delete_vm(
                                        competitor,
                                        actor.name,
                                        home=actor.home,
                                        native=partial(native, root),
                                    )
                                self.assertEqual(read_receipt(admission.root)["phase"], "starting")
                    request("unix://" + str(root / "work.sock"), ["materialize"])
                    with competitor.held(timeout=5):
                        self.assertTrue((root / "worker-done.json").exists())
                        self.assertIsNone(read_receipt(admission.root))


if __name__ == "__main__":
    unittest.main()
