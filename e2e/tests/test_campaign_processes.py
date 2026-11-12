"""Real external workload cleanup precedes another campaign's admission."""

from contextlib import ExitStack, contextmanager
import contextlib
import errno
from functools import partial
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

from e2e.campaign import Admission, main as campaign_main, read_receipt
from e2e.docker import Docker, LABEL, recover
from e2e.tests.fixtures.campaign_service import request
from release.cancellation import pause
from release.files import create_directory
from release.owned_process import spawn
from release.process import run
from release.progress import finalizer

ROOT = Path(__file__).resolve().parents[2]
SERVICE = Path(__file__).parent / "fixtures/campaign_service.py"


def native(argv, **options):
    return run([sys.executable, str(SERVICE), "client", *argv[1:]], **options)


def ready(path, owner=None):
    until = time.monotonic() + 5
    while not path.exists():
        if time.monotonic() >= until or owner is not None and owner.exited():
            raise RuntimeError("Fixture did not reach its control boundary")
        pause(0.01)


@contextmanager
def service(root, identity):
    create_directory(root, mode=0o700)
    with spawn(
        [sys.executable, str(SERVICE), "daemon", str(root), "--identity", identity]
    ) as owner:
        ready(root / "ready.json", owner)
        yield "unix://" + str(root / "control.sock")


def start(admission, docker):
    active = admission.begin("docker", "fixture", docker.owner())
    admission.advance(
        "creating",
        resource="container",
        name="hive-e2e-" + active["nonce"],
        container_id=None,
        create_completed=False,
    )
    identifier = docker.call(
        [
            "container",
            "create",
            "--name",
            active["owner"]["name"],
            "--label",
            LABEL + "=" + active["nonce"],
        ]
    ).stdout.strip()
    admission.advance("created", container_id=identifier, create_completed=True)
    docker.call(["container", "start", identifier])
    admission.advance("started")
    return active


COMPETITOR = """
import json,sys
from pathlib import Path
sys.path.insert(0,sys.argv[1])
import e2e
from e2e.campaign import Admission
from e2e.docker import Docker
from e2e.tests.test_campaign_processes import native
from release.files import atomic_write_json
from release.progress import entry
root,output,active=Path(sys.argv[2]),Path(sys.argv[3]),json.loads(sys.argv[4])
def main():
    try:
        with Admission(root).held(timeout=0):
            blocked=False
    except (TimeoutError,RuntimeError):
        blocked=True
    atomic_write_json(output/'blocked.json',dict(blocked=blocked))
    with Admission(root).held(timeout=5):
        owner=active['owner']
        row=Docker(owner['endpoint'],owner['daemon_id'],native=native).owned_container(active)
        atomic_write_json(output/'admitted.json',dict(container_exists=row is not None))
raise SystemExit(entry(main,name='scenario'))
"""


NATIVE_PARENT = """
import os,sys,time
from pathlib import Path
sys.path.insert(0,sys.argv[1])
import e2e
from e2e.campaign import Admission
from release.cancellation import pause
from release.files import atomic_write_json
from release.owned_process import spawn
from release.progress import entry
root=Path(sys.argv[2])
def main():
    admission=Admission(root/'admission')
    with admission.held():
        admission.begin('native','parent-death-fixture',dict(pid=os.getpid()))
        with spawn([sys.executable,sys.argv[3],'native-child',str(root)],pass_fds=(admission.lock_fd,)) as child:
            until=time.monotonic()+5
            while not (root/'native-ready.json').exists():
                if child.exited() or time.monotonic()>=until:raise RuntimeError('Child did not start')
                pause(.01)
            atomic_write_json(root/'parent-ready.json',dict(ready=True))
            child.wait(timeout=20)
        admission.complete()
raise SystemExit(entry(main,name='scenario'))
"""


def process_absent(pid):
    until = time.monotonic() + 5
    while True:
        gone = False
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            gone = True
        if time.monotonic() >= until:
            raise RuntimeError("Fixture process did not exit")
        if gone:
            return
        pause(0.01)


class CampaignProcessTests(unittest.TestCase):
    def test_stop_timeout_still_retires_actual_daemon_workload(self):
        with tempfile.TemporaryDirectory(prefix="hive-owner-", dir="/tmp") as temporary:
            root = Path(temporary)
            with service(root / "daemon", "daemon-a") as endpoint:
                admission = Admission(root / "admission")
                with admission.held():
                    active = start(admission, Docker(endpoint, "daemon-a", native=native))
                    pid = json.loads((root / "daemon/workload.json").read_text())["pid"]
                    failure = subprocess.TimeoutExpired(["fixture", "stop"], 1)

                    def stop_timeout(argv, **options):
                        if argv[3:5] == ["container", "stop"]:
                            raise failure
                        return native(argv, **options)

                    with self.assertRaises(subprocess.TimeoutExpired) as raised:
                        Docker(endpoint, "daemon-a", native=stop_timeout).cleanup_container(active)
                    self.assertIs(raised.exception, failure)
                    process_absent(pid)
                    self.assertEqual(request(endpoint, ["container", "ls"])["stdout"], "")
                    admission.complete()

    def test_delayed_create_materializes_after_empty_recovery_and_client_death(self):
        with (
            tempfile.TemporaryDirectory(prefix="hive-owner-", dir="/tmp") as temporary,
            contextlib.redirect_stdout(io.StringIO()),
        ):
            root = Path(temporary)
            with service(root / "daemon", "daemon-a") as endpoint:
                request(endpoint, ["defer-create"])
                admission = Admission(root / "admission")
                with self.assertRaises(RuntimeError), admission.held():
                    active = admission.begin(
                        "docker", "fixture", Docker(endpoint, "daemon-a").owner()
                    )
                    admission.advance(
                        "creating",
                        resource="container",
                        name="hive-e2e-" + active["nonce"],
                        container_id=None,
                        create_completed=False,
                    )
                    with spawn(
                        [
                            sys.executable,
                            str(SERVICE),
                            "client",
                            "--host",
                            endpoint,
                            "container",
                            "create",
                            "--name",
                            active["owner"]["name"],
                            "--label",
                            LABEL + "=" + active["nonce"],
                        ]
                    ) as client:
                        ready(root / "daemon/pending.json", client)
                        os.kill(client.pid, signal.SIGKILL)
                        self.assertEqual(client.wait(timeout=2), -signal.SIGKILL)
                recovery = partial(recover, native=native)
                with (
                    self.assertRaises(RuntimeError),
                    Admission(root / "admission", recovery).held(),
                ):
                    self.fail("An empty lookup settled an in-flight daemon request")
                self.assertEqual(read_receipt(root / "admission")["nonce"], active["nonce"])
                request(endpoint, ["materialize"])
                pid = json.loads((root / "daemon/workload.json").read_text())["pid"]
                os.kill(pid, 0)
                with Admission(root / "admission", recovery).held():
                    self.assertIsNone(read_receipt(root / "admission"))
                process_absent(pid)

    def test_two_daemons_recover_original_live_owner_despite_context_change(self):
        with (
            tempfile.TemporaryDirectory(prefix="hive-owner-", dir="/tmp") as temporary,
            contextlib.redirect_stdout(io.StringIO()),
        ):
            root = Path(temporary)
            with service(root / "a", "daemon-a") as a, service(root / "b", "daemon-b") as b:
                admission = Admission(root / "admission")
                with self.assertRaises(RuntimeError), admission.held():
                    start(admission, Docker(a, "daemon-a", native=native))
                pid = json.loads((root / "a/workload.json").read_text())["pid"]
                self.assertEqual(request(b, ["container", "ls"])["stdout"], "")
                calls = []

                def unavailable(argv, **options):
                    calls.append(argv[2])
                    raise ConnectionError("Recorded daemon is unreachable")

                with patch.dict(os.environ, DOCKER_HOST=b, DOCKER_CONTEXT="other"):
                    with (
                        self.assertRaises(ConnectionError),
                        Admission(root / "admission", partial(recover, native=unavailable)).held(),
                    ):
                        self.fail("An empty second daemon settled the original workload")
                    os.kill(pid, 0)
                    self.assertTrue(calls and all(endpoint == a for endpoint in calls))
                    self.assertEqual(read_receipt(root / "admission")["owner"]["endpoint"], a)
                    with Admission(root / "admission", partial(recover, native=native)).held():
                        self.assertIsNone(read_receipt(root / "admission"))
                process_absent(pid)
                self.assertEqual(request(b, ["container", "ls"])["stdout"], "")

    def test_native_parent_death_retains_intent_after_surviving_child_exits(self):
        with (
            tempfile.TemporaryDirectory(prefix="hive-owner-", dir="/tmp") as temporary,
            contextlib.redirect_stdout(io.StringIO()),
        ):
            root = Path(temporary)

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
                    [
                        sys.executable,
                        "-c",
                        NATIVE_PARENT,
                        str(ROOT),
                        str(root),
                        str(SERVICE),
                    ]
                ) as parent,
            ):
                ready(root / "parent-ready.json", parent)
                active = read_receipt(root / "admission")
                os.kill(parent.pid, signal.SIGKILL)
                self.assertEqual(parent.wait(timeout=2), -signal.SIGKILL)
                with (
                    self.assertRaises(TimeoutError),
                    Admission(root / "admission").held(timeout=0.1),
                ):
                    self.fail("A surviving child lost its inherited admission lock")
                retire()
                with self.assertRaises(RuntimeError), Admission(root / "admission").held(timeout=1):
                    self.fail("Native parent PID absence silently cleared active intent")
                self.assertEqual(read_receipt(root / "admission")["nonce"], active["nonce"])
                with (
                    patch("e2e.campaign.default_root", return_value=root / "admission"),
                    patch(
                        "sys.argv",
                        [
                            "campaign",
                            "clear",
                            "--nonce",
                            active["nonce"],
                            "--manual-cleanup-confirmed",
                        ],
                    ),
                ):
                    campaign_main()
                with Admission(root / "admission").held():
                    pass

    def test_killed_client_leaves_real_workload_until_cleanup_then_competitor_enters(self):
        with tempfile.TemporaryDirectory(prefix="hive-owner-", dir="/tmp") as temporary:
            root = Path(temporary)
            with service(root / "daemon", "daemon-a") as endpoint, ExitStack() as owners:
                admission = Admission(root / "admission")
                with admission.held():
                    docker = Docker(endpoint, "daemon-a", native=native)
                    active = start(admission, docker)
                    pid = json.loads((root / "daemon/workload.json").read_text())["pid"]
                    with spawn(
                        [
                            sys.executable,
                            str(SERVICE),
                            "client",
                            "--host",
                            endpoint,
                            "container",
                            "exec",
                            active["owner"]["container_id"],
                        ]
                    ) as client:
                        ready(root / "daemon/executing.json", client)
                        os.kill(client.pid, signal.SIGKILL)
                        self.assertEqual(client.wait(timeout=2), -signal.SIGKILL)
                    self.assertTrue(docker.owned_container(active)["State"]["Running"])
                    competitor = owners.enter_context(
                        spawn(
                            [
                                sys.executable,
                                "-c",
                                COMPETITOR,
                                str(ROOT),
                                str(root / "admission"),
                                str(root),
                                json.dumps(active),
                            ]
                        )
                    )
                    ready(root / "blocked.json", competitor)
                    self.assertTrue(json.loads((root / "blocked.json").read_text())["blocked"])
                    self.assertFalse((root / "admitted.json").exists())
                    result = docker.cleanup_container(active)
                    self.assertTrue(result["confirmed"])
                    admission.complete()
                ready(root / "admitted.json", competitor)
                self.assertEqual(competitor.wait(timeout=2), 0)
                self.assertFalse(
                    json.loads((root / "admitted.json").read_text())["container_exists"]
                )
                with self.assertRaises(ProcessLookupError) as absent:
                    os.kill(pid, 0)
                self.assertEqual(absent.exception.errno, errno.ESRCH)

    def test_failed_external_cleanup_retains_intent_and_refuses_competitor(self):
        with (
            tempfile.TemporaryDirectory(prefix="hive-owner-", dir="/tmp") as temporary,
            contextlib.redirect_stdout(io.StringIO()),
        ):
            root = Path(temporary)
            with service(root / "daemon", "daemon-a") as endpoint:
                admission = Admission(root / "admission")
                with self.assertRaises(RuntimeError), admission.held():
                    docker = Docker(endpoint, "daemon-a", native=native)
                    active = start(admission, docker)
                    request(endpoint, ["refuse-cleanup"])
                    docker.cleanup_container(active)
                self.assertEqual(read_receipt(root / "admission")["nonce"], active["nonce"])
                with (
                    self.assertRaises(RuntimeError),
                    Admission(root / "admission", partial(recover, native=native)).held(),
                ):
                    self.fail("An unconfirmed external workload was admitted")
                request(endpoint, ["allow-cleanup"])
                with Admission(root / "admission", partial(recover, native=native)).held():
                    self.assertIsNone(read_receipt(root / "admission"))
