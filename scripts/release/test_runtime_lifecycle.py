"""Lock/network/cleanup fault tests without real providers or external downloads."""

from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import Mock, patch
from urllib.parse import urlsplit
from urllib.request import Request, urlopen
import check_compatibility
import check_release_install
from .acceptance import ready, running, wait_for
from .cancellation import pause
from .download import fetch
from .embedding_fixture import embedding_server
from . import embedding_fixture
from .locking import directory_lock
from .owned_process import spawn
from .process import ProcessFailure, run
from . import files
from .progress import entry
from .temporary import directory as private_directory
from .unix_transport import exchange


class RuntimeLifecycleTests(unittest.TestCase):
    def test_service_cleanup_keeps_the_native_service_grace(self):
        code = """
import signal,sys,time
def retire(*args):
    time.sleep(.7)
    sys.exit(0)
signal.signal(signal.SIGINT,retire)
print('ready',flush=True)
while True:time.sleep(.1)
"""
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "service.log"
            with running([sys.executable, "-c", code], log, dict(os.environ)):
                wait_for(lambda: log.read_bytes() == b"ready\n", "fixture service", seconds=2)

    def test_startup_retries_explicit_unavailability_then_returns_ready_value(self):
        for code in ("daemon_unavailable", "comb_unavailable"):
            failure = ProcessFailure(
                subprocess.CompletedProcess([], 1, json.dumps({"error": {"code": code}}), b"")
            )
            call = Mock(side_effect=[failure, failure, {"ready": True}])
            with self.subTest(code=code):
                self.assertEqual(
                    wait_for(lambda: ready(call, "stats"), "fixture startup", seconds=2),
                    {"ready": True},
                )
                self.assertEqual(call.call_count, 3)

    def test_unix_reply_and_signal_during_stalled_reply(self):
        with tempfile.TemporaryDirectory() as directory, socket.socket(socket.AF_UNIX) as listener:
            root = Path(directory)
            address = root / "rpc"
            listener.bind(str(address))
            listener.listen(2)
            listener.settimeout(2)

            def answer():
                client, _ = listener.accept()
                with client:
                    client.settimeout(1)
                    client.recv(4096)
                    client.sendall(b'{"ok":true}\n')

            worker = threading.Thread(target=answer)
            worker.start()
            try:
                self.assertEqual(exchange(address, {"request": "fixture"}), {"ok": True})
            finally:
                worker.join(1)
            self.assertFalse(worker.is_alive())
            code = """
import sys
from release.progress import entry
from release.unix_transport import exchange
sys.exit(entry(lambda:exchange(sys.argv[1],{'request':'fixture'}),name='verify'))
"""
            with spawn(
                [sys.executable, "-c", code, str(address)],
                env=dict(os.environ, PYTHONPATH="scripts"),
            ) as child:
                client, _ = listener.accept()
                with client:
                    client.settimeout(1)
                    self.assertTrue(client.recv(4096))
                    began = time.monotonic()
                    child._signal(signal.SIGTERM)
                    self.assertEqual(child.wait(timeout=4), 143)
                    self.assertLess(time.monotonic() - began, 5)

    def test_mock_publication_copy_stays_standalone_after_lock_policy_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "record"
            code = """
import importlib.machinery,importlib.util,sys
from pathlib import Path
loader=importlib.machinery.SourceFileLoader('mock_files',sys.argv[1])
spec=importlib.util.spec_from_loader(loader.name,loader)
module=importlib.util.module_from_spec(spec)
loader.exec_module(module)
module.append_jsonl_record(Path(sys.argv[2]),{'ok':True})
"""
            run([sys.executable, "-c", code, files.__file__, str(output)], timeout=2, check=True)
            self.assertEqual(json.loads(output.read_bytes()), {"ok": True})

    def test_background_http_failure_is_observed_during_cleanup(self):
        with patch.object(embedding_fixture, "serve", side_effect=ValueError("synthetic worker")):
            with self.assertRaisesRegex(RuntimeError, "thread failed"):
                with embedding_server():
                    pass
        self.assertFalse(any(t.name == "hive-fixture-http" for t in threading.enumerate()))

    def test_uncertain_child_cleanup_retains_its_temporary_root(self):
        with tempfile.TemporaryDirectory() as parent:
            primary = OSError("uncertain synthetic child")
            primary.owned_cleanup_uncertain = True
            with self.assertRaises(OSError) as caught:
                with private_directory(prefix="retained-", parent=parent) as path:
                    raise primary
            self.assertIs(caught.exception, primary)
            self.assertTrue(path.exists())
            self.assertEqual(primary.retained_root, str(path))

    def test_embedding_fixture_returns_vectors_and_retires_a_stalled_client(self):
        with embedding_server() as url:
            request = Request(
                url,
                data=json.dumps({"input": ["one", "two"]}).encode(),
                headers={"Content-Type": "application/json"},
            )
            with urlopen(request, timeout=2) as response:
                self.assertEqual(len(json.loads(response.read())["data"]), 2)
            client = socket.create_connection(("127.0.0.1", urlsplit(url).port), timeout=1)
            client.sendall(b"POST /v1/embeddings HTTP/1.1\r\nContent-Length: 100000\r\n\r\nx")
            began = time.monotonic()
        client.close()
        self.assertLess(time.monotonic() - began, 0.5)
        self.assertFalse(any(t.name == "hive-fixture-http" for t in threading.enumerate()))

    def test_signal_interrupts_a_contended_publication_lock(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            marker, report = root / "ready", root / "report"
            code = """
import sys
from pathlib import Path
from release.locking import directory_lock
from release.cancellation import finish
from release.progress import entry
def work():
    Path(sys.argv[2]).write_text('ready')
    with directory_lock(Path(sys.argv[1])):
        finish(lambda:Path(sys.argv[3]).write_text('passed'))
sys.exit(entry(work,name='verify'))
"""
            with (
                directory_lock(root),
                spawn(
                    [sys.executable, "-c", code, str(root), str(marker), str(report)],
                    env=dict(os.environ, PYTHONPATH="scripts"),
                ) as child,
            ):
                self.until(marker.exists)
                began = time.monotonic()
                child._signal(signal.SIGTERM)
                self.assertEqual(child.wait(timeout=4), 143)
                self.assertLess(time.monotonic() - began, 5)
            self.assertFalse(report.exists())

    def until(self, predicate):
        deadline = time.monotonic() + 2
        while not predicate() and time.monotonic() < deadline:
            pause(0.02)
        self.assertTrue(predicate())

    @contextmanager
    def http_fixture(self, *, stalled=False):
        stop, requested = threading.Event(), threading.Event()
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen(1)
            listener.settimeout(0.1)

            def serve():
                while not stop.is_set():
                    try:
                        client, _ = listener.accept()
                    except TimeoutError:
                        continue
                    with client:
                        client.settimeout(0.1)
                        client.recv(4096)
                        client.sendall(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\n"
                        )
                        requested.set()
                        if stalled:
                            stop.wait(3)
                        else:
                            client.sendall(b"fixture")
                    return

            worker = threading.Thread(target=serve)
            worker.start()
            try:
                yield f"http://127.0.0.1:{listener.getsockname()[1]}/body", requested
            finally:
                stop.set()
                worker.join(1)
                self.assertFalse(worker.is_alive())

    def test_owned_download_checks_actual_bytes_and_declared_pin(self):
        with self.http_fixture() as (url, _):
            self.assertEqual(
                fetch(url, hashlib.sha256(b"fixture").hexdigest(), local=True), b"fixture"
            )
        with self.http_fixture() as (url, _), self.assertRaisesRegex(ValueError, "SHA-256 pin"):
            fetch(url, "0" * 64, local=True)

    def test_signal_kills_stalled_download_before_temporary_cleanup_and_report(self):
        with (
            tempfile.TemporaryDirectory() as directory,
            self.http_fixture(stalled=True) as (url, requested),
        ):
            root = Path(directory)
            marker, report = root / "worker", root / "report"
            code = """
import sys
from pathlib import Path
from release import owned_process
from release.download import fetch
from release.cancellation import finish
from release.progress import entry
original=owned_process.Owned.__init__
def adopted(self,*args,**kwargs):
    original(self,*args,**kwargs)
    Path(sys.argv[2]).write_text(str(self.pid))
owned_process.Owned.__init__=adopted
def work():
    fetch(sys.argv[1],'0'*64,local=True)
    finish(lambda:Path(sys.argv[3]).write_text('passed'))
sys.exit(entry(work,name='fetch'))
"""
            with spawn(
                [sys.executable, "-c", code, url, str(marker), str(report)],
                env=dict(os.environ, PYTHONPATH="scripts"),
            ) as child:
                self.until(lambda: marker.exists() and requested.is_set())
                worker = int(marker.read_text())
                began = time.monotonic()
                child._signal(signal.SIGTERM)
                self.assertEqual(child.wait(timeout=4), 143)
                self.assertLess(time.monotonic() - began, 5)
            self.assertFalse(report.exists())
            with self.assertRaises(ProcessLookupError):
                os.killpg(worker, 0)

    def test_bad_readiness_reply_is_not_retried_and_only_connection_errors_are_transient(self):
        malformed = Mock(side_effect=json.JSONDecodeError("invalid", "", 0))
        with self.assertRaises(json.JSONDecodeError):
            wait_for(malformed, "fixture", seconds=1)
        malformed.assert_called_once()
        for code, transient in (("daemon_unavailable", True), ("invalid_request", False)):
            failure = ProcessFailure(
                subprocess.CompletedProcess([], 1, json.dumps({"error": {"code": code}}), b"")
            )
            if transient:
                self.assertIsNone(ready(Mock(side_effect=failure)))
            else:
                with self.assertRaises(ProcessFailure):
                    ready(Mock(side_effect=failure))

    def test_cleanup_failure_prevents_installed_passing_report(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            @contextmanager
            def broken_cleanup(**kwargs):
                yield root / "scratch"
                raise OSError("synthetic cleanup failure")

            checks = Mock(identities={})
            for name in ("core", "hosted", "wizard"):
                getattr(checks, name).return_value = {}
            with (
                patch.object(check_release_install, "private_directory", broken_cleanup),
                patch.object(check_release_install, "InstalledChecks", return_value=checks),
                patch.object(
                    sys,
                    "argv",
                    ["check", "--assets", str(root), "--output-dir", str(root / "output")],
                ),
            ):
                self.assertEqual(entry(check_release_install.main, name="verify"), 1)
            self.assertFalse((root / "output/results.json").exists())

    def test_nested_debug_or_missing_release_binary_is_refused_before_any_probe(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for path in (root / "release/target/debug", root / "release"):
                path.mkdir(parents=True, exist_ok=True)
                args = Mock(current_dir=path, output_dir=root / "output")
                with (
                    self.subTest(path=path),
                    patch.object(check_compatibility, "identity") as probe,
                ):
                    with self.assertRaises(RuntimeError):
                        check_compatibility.admit(args)
                    probe.assert_not_called()
                self.assertFalse((root / "output").exists())
