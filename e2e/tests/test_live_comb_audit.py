"""Actual bounded local proxy IO, independent of embeddings and provider accounts."""

import json
from pathlib import Path
import socket
import socketserver
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from e2e.live.audit import AuditProxy, FrameError, receive
from e2e.scripts.harness.worker import Worker
from release.cancellation import pause, scope
from release.progress import cleanup_all, finalizer


class ProxyTests(unittest.TestCase):
    def test_large_reply_and_overflow_keep_complete_evidence_without_credentials(self):
        for limit in (128 * 1024**2, 1024):
            with (
                self.subTest(limit=limit),
                tempfile.TemporaryDirectory(prefix="hive-audit-", dir="/tmp") as temporary,
                scope(),
            ):
                root = Path(temporary)
                response = json.dumps({"result": "€" * (400 * 1024)}).encode() + b"\n"

                class Handler(socketserver.BaseRequestHandler):
                    def handle(self):
                        receive(self.request, 8192, time.monotonic() + 5)
                        self.request.sendall(response)

                server = socketserver.UnixStreamServer(str(root / "upstream"), Handler)
                server.timeout = 0.1
                worker = Worker(server.handle_request, stop=threading.Event())
                worker.start()
                proxy = AuditProxy(root / "proxy", root / "upstream", root / "audit")

                def retire_proxy():
                    if limit > len(response):
                        proxy.stop()
                    else:
                        with self.assertRaisesRegex(RuntimeError, "retirement"):
                            proxy.stop()

                with (
                    finalizer(lambda: cleanup_all(retire_proxy, worker.join, server.server_close)),
                    patch("e2e.live.audit.AUDIT_RECORD_LIMIT", limit),
                ):
                    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
                        client.settimeout(5)
                        client.connect(str(root / "proxy"))
                        client.sendall(
                            json.dumps(
                                dict(
                                    claim={"id": "agent"},
                                    operation={"op": "get"},
                                    credential="secret-not-logged",
                                )
                            ).encode()
                            + b"\n"
                        )
                        with client.makefile("rb") as stream:
                            actual = stream.readline()
                    if limit > len(response):
                        self.assertEqual(actual, response)
                        self.assertEqual(proxy.rows()[0]["reply"], json.loads(response))
                        self.assertNotIn("secret-not-logged", (root / "audit").read_text())
                        self.assertFalse(proxy.errors)
                    else:
                        self.assertEqual(actual, b"")
                        self.assertTrue(any("exceeds" in error for error in proxy.errors))
                        self.assertFalse((root / "audit").exists())
                self.assertFalse(worker.is_alive())
                self.assertFalse(proxy.owner.is_alive())
                self.assertTrue(all(not w.is_alive() for w in proxy.workers))

    def test_late_handler_failure_prevents_acceptance_and_retains_admission(self):
        from contextlib import ExitStack
        from types import SimpleNamespace
        from unittest.mock import Mock
        from e2e.campaign import read_receipt
        from e2e.scripts.run_live_comb import Scenario
        from e2e.tests.test_live_ownership import state_fixture
        from release.progress import entry

        with state_fixture() as root:
            scenario = object.__new__(Scenario)
            scenario.stack = ExitStack()
            scenario.args = SimpleNamespace(state_dir=root, output_dir=root, timeout=10)
            scenario.output, scenario.agents, scenario.pending_spawns = root, [], set()
            scenario.collector = None
            scenario.daemon_owner = SimpleNamespace(ready=False, close=Mock())
            scenario.comb_process = SimpleNamespace(stop=Mock(), returncode=0)
            began = threading.Event()

            def handler(peer):
                began.set()
                if not peer.server.stop_event.wait(5):
                    raise TimeoutError("Fixture handler did not retire")
                with peer.server.lock:
                    peer.server.failures.append("late_fixture_failure")

            def startup():
                scenario.admission.advance("starting")
                scenario.proxy = AuditProxy(root / "audit.sock", root / "absent", root / "audit")

            def execute():
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
                    client.settimeout(5)
                    client.connect(str(root / "audit.sock"))
                    if not began.wait(5):
                        raise TimeoutError("Fixture handler did not begin")
                    self.assertFalse(scenario.proxy.errors)
                return {}

            scenario.startup, scenario.execute = startup, execute
            with (
                patch("e2e.live.audit.AuditHandler.handle", handler),
                patch("e2e.campaign.default_root", return_value=root / "admission"),
            ):
                self.assertEqual(entry(scenario.run, name="scenario"), 1)
            self.assertEqual(read_receipt(root / "admission")["phase"], "starting")
            self.assertFalse((root / "result.json").exists())
            self.assertTrue((root / "retained.json").exists())
            scenario.daemon_owner.close.assert_called_once()
            scenario.comb_process.stop.assert_called_once()
            self.assertTrue(all(not w.is_alive() for w in scenario.proxy.workers))

    def test_partial_peer_is_cancelled_and_joined_without_waiting_for_frame_deadline(self):
        with tempfile.TemporaryDirectory(prefix="hive-audit-", dir="/tmp") as temporary, scope():
            root = Path(temporary)
            proxy = AuditProxy(root / "proxy", root / "absent", root / "audit")
            with finalizer(proxy.stop), socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
                client.settimeout(5)
                client.connect(str(root / "proxy"))
                client.sendall(b'{"claim":')
                until = time.monotonic() + 5
                while not proxy.workers:
                    if time.monotonic() >= until:
                        self.fail("Proxy fixture did not accept its peer")
                    pause(0.01)
                began = time.monotonic()
                proxy.stop()
                self.assertLess(time.monotonic() - began, 2)
                self.assertTrue(all(not w.is_alive() for w in proxy.workers))
                self.assertFalse((root / "audit").exists())

    def test_frame_boundaries_size_and_deadline_are_strict(self):
        for frame in (b"{}", b"{}\n{}\n", b"123456\n"):
            with self.subTest(frame=frame), scope():
                left, right = socket.socketpair()
                with left, right:
                    left.sendall(frame)
                    left.shutdown(socket.SHUT_WR)
                    with self.assertRaises(FrameError):
                        receive(right, 6, time.monotonic() + 1)
        with scope():
            left, right = socket.socketpair()
            with left, right:
                with self.assertRaises(TimeoutError):
                    receive(right, 10, time.monotonic() + 0.02)
