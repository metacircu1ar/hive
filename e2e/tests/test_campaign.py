"""Campaign intent survives failed recovery and cannot follow a changed daemon."""

import contextlib
from functools import partial
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from e2e.campaign import Admission, main, read_receipt
from e2e.docker import Docker, LABEL, recover
from release.files import atomic_write_text
from release.process import ProcessFailure, checked


class NativeDocker:
    def __init__(self):
        self.identities = {"unix:///a.sock": "daemon-a", "unix:///b.sock": "daemon-b"}
        self.rows = {}
        self.calls = []
        self.lookup_fails = False

    def __call__(self, argv, **options):
        self.calls.append((argv, options.get("env", {})))
        endpoint, args = argv[2], argv[3:]
        if endpoint not in self.identities:
            raise ConnectionError("Recorded daemon is unavailable")
        status = 0
        if args[0] == "info":
            body = dict(ID=self.identities[endpoint])
        elif args[:2] == ["container", "ls"]:
            status = 1 if self.lookup_fails else 0
            row = self.rows.get(endpoint)
            body = None if row is None else dict(ID=row["Id"])
            text = "" if body is None else json.dumps(body) + "\n"
        elif args[:2] == ["container", "inspect"]:
            body = [self.rows[endpoint]]
        elif args[:2] in (["container", "stop"], ["container", "kill"]):
            self.rows[endpoint]["State"]["Running"] = False
            body = {}
        elif args[:2] == ["container", "rm"]:
            del self.rows[endpoint]
            body = {}
        else:
            raise AssertionError("Unexpected fixture Docker command")
        if args[:2] != ["container", "ls"]:
            text = json.dumps(body)
        result = subprocess.CompletedProcess(argv, status, text, "")
        return checked(result) if options.get("check") else result

    def create(self, active):
        self.rows[active["owner"]["endpoint"]] = dict(
            Id="a" * 64,
            Name="/" + active["owner"]["name"],
            Config=dict(Labels={LABEL: active["nonce"]}),
            State=dict(Running=True),
        )


def creating(admission, completed=False):
    active = admission.begin("docker", "fixture", Docker("unix:///a.sock", "daemon-a").owner())
    admission.advance(
        "created" if completed else "creating",
        resource="container",
        name="hive-e2e-" + active["nonce"],
        container_id="a" * 64 if completed else None,
        create_completed=completed,
    )
    return active


class CampaignTests(unittest.TestCase):
    def test_failed_forced_cleanup_keeps_original_stop_error_and_intent(self):
        with tempfile.TemporaryDirectory() as temporary, contextlib.redirect_stdout(io.StringIO()):
            native = NativeDocker()
            admission = Admission(Path(temporary))
            with self.assertRaises(OSError) as raised, admission.held():
                active = creating(admission, completed=True)
                native.create(active)
                failure = OSError("Fixture original stop failure")

                def failed_stop(argv, **options):
                    if argv[3:5] == ["container", "stop"]:
                        native.lookup_fails = True
                        raise failure
                    return native(argv, **options)

                Docker("unix:///a.sock", "daemon-a", native=failed_stop).cleanup_container(active)
            self.assertIs(raised.exception, failure)
            self.assertTrue(
                any(isinstance(error, ProcessFailure) for error in failure.cleanup_errors)
            )
            self.assertIsNotNone(read_receipt(Path(temporary)))

    def test_raised_stop_still_attempts_inspection_kill_and_removal(self):
        for error in (
            OSError("Fixture stop transport failure"),
            subprocess.TimeoutExpired(["fixture"], 1),
        ):
            with (
                self.subTest(error=type(error).__name__),
                tempfile.TemporaryDirectory() as temporary,
            ):
                native = NativeDocker()
                admission = Admission(Path(temporary))
                with admission.held():
                    active = creating(admission, completed=True)
                    native.create(active)

                    def failed_stop(argv, **options):
                        if argv[3:5] == ["container", "stop"]:
                            raise error
                        return native(argv, **options)

                    with self.assertRaises(type(error)) as failure:
                        Docker("unix:///a.sock", "daemon-a", native=failed_stop).cleanup_container(
                            active
                        )
                    self.assertIs(failure.exception, error)
                    self.assertNotIn("unix:///a.sock", native.rows)
                    self.assertTrue(
                        any(argv[3:5] == ["container", "kill"] for argv, _ in native.calls)
                    )
                    self.assertTrue(
                        any(argv[3:5] == ["container", "rm"] for argv, _ in native.calls)
                    )
                    admission.complete()

    def test_native_intent_blocks_and_manual_clear_requires_exact_nonce(self):
        with (
            tempfile.TemporaryDirectory() as temporary,
            contextlib.redirect_stdout(io.StringIO()) as output,
        ):
            root = Path(temporary)
            first = Admission(root)
            with self.assertRaises(RuntimeError), first.held():
                active = first.begin("native", "fixture", dict(pid=123))
            with self.assertRaises(RuntimeError), Admission(root).held():
                self.fail("Native ownership was inferred from a PID")
            self.assertIn(active["nonce"], output.getvalue())
            with patch("e2e.campaign.default_root", return_value=root):
                with patch(
                    "sys.argv",
                    ["campaign", "clear", "--nonce", "wrong", "--manual-cleanup-confirmed"],
                ):
                    with self.assertRaises(ValueError):
                        main()
                self.assertEqual(read_receipt(root)["nonce"], active["nonce"])
                with patch(
                    "sys.argv",
                    ["campaign", "clear", "--nonce", active["nonce"], "--manual-cleanup-confirmed"],
                ):
                    main()
            with Admission(root).held() as next_owner:
                self.assertIsNone(next_owner.active)

    def test_empty_list_cannot_settle_delayed_creation(self):
        native = NativeDocker()
        with tempfile.TemporaryDirectory() as temporary, contextlib.redirect_stdout(io.StringIO()):
            root = Path(temporary)
            first = Admission(root)
            with self.assertRaises(RuntimeError), first.held():
                active = creating(first)
            recovery = partial(recover, native=native)
            with self.assertRaises(RuntimeError), Admission(root, recovery).held():
                self.fail("Unresolved create was settled by the first empty list")
            self.assertEqual(read_receipt(root)["nonce"], active["nonce"])
            native.create(active)
            with Admission(root, recovery).held():
                self.assertNotIn("unix:///a.sock", native.rows)
            self.assertIsNone(read_receipt(root))

    def test_recovery_uses_original_daemon_despite_changed_environment(self):
        native = NativeDocker()
        with tempfile.TemporaryDirectory() as temporary, contextlib.redirect_stdout(io.StringIO()):
            root = Path(temporary)
            first = Admission(root)
            with self.assertRaises(RuntimeError), first.held():
                active = creating(first, completed=True)
            native.create(active)
            native.identities.pop("unix:///a.sock")
            recovery = partial(recover, native=native)
            with patch.dict("os.environ", DOCKER_HOST="unix:///b.sock", DOCKER_CONTEXT="other"):
                with self.assertRaises(ConnectionError), Admission(root, recovery).held():
                    self.fail("Daemon B's absence settled daemon A's workload")
                self.assertIn("unix:///a.sock", native.rows)
                native.identities["unix:///a.sock"] = "daemon-a"
                with Admission(root, recovery).held():
                    pass
            self.assertTrue(all(argv[2] == "unix:///a.sock" for argv, _ in native.calls))
            self.assertTrue(
                all(
                    "DOCKER_HOST" not in env and "DOCKER_CONTEXT" not in env
                    for _, env in native.calls
                )
            )

    def test_failed_lookup_and_changed_label_do_not_prove_absence(self):
        native = NativeDocker()
        with tempfile.TemporaryDirectory() as temporary, contextlib.redirect_stdout(io.StringIO()):
            root = Path(temporary)
            first = Admission(root)
            with self.assertRaises(RuntimeError), first.held():
                active = creating(first, completed=True)
            native.create(active)
            native.lookup_fails = True
            with self.assertRaises(ProcessFailure):
                recover(active, native=native)
            native.lookup_fails = False
            native.rows["unix:///a.sock"]["Config"]["Labels"][LABEL] = "foreign"
            with self.assertRaises(ValueError):
                recover(active, native=native)
            self.assertFalse(any(argv[3:5] == ["container", "stop"] for argv, _ in native.calls))

    def test_strict_receipt_refuses_duplicate_fields_and_boolean_format(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for data in ('{"format":1,"format":1,"active":null}', '{"format":true,"active":null}'):
                atomic_write_text(root / "campaign.json", data)
                with self.assertRaises(ValueError):
                    read_receipt(root)
