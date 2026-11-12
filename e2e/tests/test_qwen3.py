"""Offline model cache, inherited lock and CPU/Metal admission regressions."""

import fcntl
import errno
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import io
from contextlib import redirect_stdout
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from e2e import cache
from e2e.models import qwen3 as qwen
from e2e.paths import ROOT
from release.cancellation import pause, scope
from release.files import atomic_write_bytes, atomic_write_text
from release.owned_process import spawn
from release.process import ProcessFailure


def pin(data):
    return {**qwen.PIN, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}


class DownloadPublication(unittest.TestCase):
    def test_missing_cache_is_private_under_normal_umasks_and_existing_public_cache_is_refused(
        self,
    ):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for mask in (0o022, 0o002):
                previous = os.umask(mask)
                try:
                    selected = root / f"cache-{mask}"
                    self.assertEqual(cache.cache_directory(selected), selected)
                    self.assertEqual(selected.stat().st_mode & 0o777, 0o700)
                    public = root / f"public-{mask}"
                    public.mkdir()
                    permissions = public.stat().st_mode & 0o777
                    self.assertNotEqual(permissions, 0o700)
                    with self.assertRaises(ValueError):
                        cache.cache_directory(public)
                    self.assertEqual(public.stat().st_mode & 0o777, permissions)
                finally:
                    os.umask(previous)

    def test_model_verifier_does_not_require_python_file_digest(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "model"
            atomic_write_bytes(path, b"model")
            with (
                patch.object(qwen, "PIN", pin(b"model")),
                patch.object(hashlib, "file_digest", None, create=True),
            ):
                qwen.verify(path)

    def test_resume_writes_checked_inode_then_publishes_verified_model(self):
        data = b"test-model"
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            partial = root / (qwen.FILENAME + ".part")
            atomic_write_text(partial, "test-")
            inode = partial.stat().st_ino

            def curl(argv, **options):
                self.assertEqual(argv[argv.index("--continue-at") + 1], "5")
                self.assertEqual(argv[argv.index("--output") + 1], "-")
                self.assertEqual(partial.stat().st_mode & 0o777, 0o600)
                self.assertEqual(os.fstat(options["stdout"].fileno()).st_ino, inode)
                self.assertEqual(len(options["pass_fds"]), 1)
                options["stdout"].write(b"model")
                return subprocess.CompletedProcess(argv, 0)

            with patch.object(qwen, "PIN", pin(data)), patch.object(cache, "run", side_effect=curl):
                qwen.prepare(root, 8081)
            self.assertEqual((root / qwen.FILENAME).read_bytes(), data)
            self.assertFalse(partial.exists())
            self.assertTrue((root / "manifest.json").is_file())

    def test_failed_transfer_retains_partial_without_final_name_or_retry(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)

            def curl(argv, **options):
                options["stdout"].write(b"partial")
                raise ProcessFailure(subprocess.CompletedProcess(argv, 18))

            with (
                patch.object(qwen, "PIN", pin(b"whole-model")),
                patch.object(cache, "run", side_effect=curl) as run,
            ):
                with self.assertRaises(ProcessFailure):
                    qwen.prepare(root, 8081)
                self.assertEqual(run.call_count, 1)
            self.assertFalse((root / qwen.FILENAME).exists())
            self.assertEqual((root / (qwen.FILENAME + ".part")).read_bytes(), b"partial")

    def test_foreign_partial_symlink_is_refused_before_curl(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            foreign = root / "foreign"
            foreign.write_bytes(b"keep")
            (root / (qwen.FILENAME + ".part")).symlink_to(foreign)
            with patch.object(cache, "run") as child:
                with self.assertRaises((OSError, ValueError)):
                    qwen.prepare(root, 8081)
                child.assert_not_called()
            self.assertEqual(foreign.read_bytes(), b"keep")

    def test_corrupt_download_is_not_published_and_cached_bytes_are_rehashed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)

            def curl(argv, **options):
                options["stdout"].write(b"bad!")

            with (
                patch.object(qwen, "PIN", pin(b"good")),
                patch.object(cache, "run", side_effect=curl),
            ):
                with self.assertRaises(ValueError):
                    qwen.prepare(root, 8081)
            self.assertFalse((root / qwen.FILENAME).exists())
            atomic_write_bytes(root / qwen.FILENAME, b"bad!")
            with patch.object(qwen, "PIN", pin(b"good")), patch.object(cache, "run") as child:
                with self.assertRaises(ValueError):
                    qwen.prepare(root, 8081)
                child.assert_not_called()

    def test_download_child_holds_publication_lock_after_parent_dies(self):
        child = """
import os,sys,time
from pathlib import Path
root=Path(sys.argv[1])
sys.stdout.buffer.write(b'half');sys.stdout.buffer.flush()
(root/'ready').write_text('ready')
until=time.monotonic()+10
while not (root/'release').exists() and time.monotonic()<until:time.sleep(.01)
sys.stdout.buffer.write(b'rest');sys.stdout.buffer.flush()
"""
        parent = """
import sys
from pathlib import Path
from e2e import cache
from release.process import run
from release.progress import entry
real=run
def curl(argv,**options):
    return real([sys.executable,'-c',sys.argv[2],sys.argv[1]],**options)
cache.run=curl
raise SystemExit(entry(lambda:cache.fetch(Path(sys.argv[1]),'model','https://fixture.invalid',
    dict(bytes=8,sha256='unused')),name='fetch'))
"""
        with tempfile.TemporaryDirectory() as temporary, scope():
            root = Path(temporary)
            environment = dict(
                os.environ, PYTHONPATH=str(ROOT) + os.pathsep + str(ROOT / "scripts")
            )
            with spawn([sys.executable, "-c", parent, str(root), child], env=environment) as owner:
                try:
                    until = time.monotonic() + 10
                    while not (root / "ready").exists():
                        if owner.exited() or time.monotonic() >= until:
                            self.fail("Download child did not reach its write boundary")
                        pause(0.01)
                    os.kill(owner.pid, signal.SIGKILL)
                    self.assertEqual(owner.wait(timeout=5), -signal.SIGKILL)
                    with (root / ".model-publication.lock").open("rb") as lock:
                        with self.assertRaises(BlockingIOError):
                            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                        atomic_write_text(root / "release", "")
                        until = time.monotonic() + 5
                        while True:
                            try:
                                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                                break
                            except BlockingIOError:
                                if time.monotonic() >= until:
                                    self.fail("Download child did not release its inherited lock")
                                pause(0.01)
                finally:
                    atomic_write_text(root / "release", "")


class ModelAdmission(unittest.TestCase):
    def test_unresolved_membership_cannot_use_an_unlimited_visible_parent(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            proc, group = root / "proc", root / "cgroup"
            (proc / "self").mkdir(parents=True)
            (group / "leaf").mkdir(parents=True)
            atomic_write_text(group / "cgroup.controllers", "memory cpu\n")
            atomic_write_text(proc / "self/mountinfo",
                              f"1 0 0:1 / {group} rw - cgroup2 cgroup rw\n")
            atomic_write_text(proc / "meminfo", "MemAvailable: 33554432 kB\n")
            atomic_write_text(proc / "self/cgroup", "0::/slice/leaf\n")
            atomic_write_text(group / "memory.max", "max\n")
            atomic_write_text(group / "leaf/memory.max", str(4 * 1024**3))
            atomic_write_text(group / "leaf/memory.current", "0\n")
            with self.assertRaisesRegex(ValueError, "Cannot resolve"):
                qwen.available_memory(proc)

    def test_existing_port_listener_is_refused_before_model_io_or_spawn(self):
        for family, address in (
            (socket.AF_INET, "127.0.0.1"),
            (socket.AF_INET, "0.0.0.0"),
            (socket.AF_INET6, "::"),
        ):
            with self.subTest(address=address):
                with tempfile.TemporaryDirectory() as temporary, socket.socket(family) as listener:
                    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                    if family == socket.AF_INET6:
                        listener.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 0)
                    listener.bind((address, 0))
                    listener.listen()
                    port = listener.getsockname()[1]
                    with (
                        patch.object(qwen, "available_memory", return_value=20 * 1024**3),
                        patch.object(qwen, "verify") as verify,
                        patch.object(qwen, "spawn") as spawn,
                        redirect_stdout(io.StringIO()),
                    ):
                        with self.assertRaises(OSError) as failure:
                            qwen.serve(Path(temporary), port, platform="linux")
                        self.assertEqual(failure.exception.errno, errno.EADDRINUSE)
                        verify.assert_not_called()
                        spawn.assert_not_called()

    def test_listener_started_during_model_preflight_is_refused_before_spawn(self):
        with socket.socket() as reserve:
            reserve.bind(("127.0.0.1", 0))
            port = reserve.getsockname()[1]
        with tempfile.TemporaryDirectory() as temporary, socket.socket() as listener:

            def start_listener(_):
                listener.bind(("127.0.0.1", port))
                listener.listen()

            with (
                patch.object(qwen, "available_memory", return_value=20 * 1024**3),
                patch.object(qwen, "verify", side_effect=start_listener),
                patch.object(qwen, "executable", return_value=Path(sys.executable)),
                patch.object(qwen, "run"),
                patch.object(qwen, "spawn") as spawn,
                redirect_stdout(io.StringIO()),
            ):
                with self.assertRaises(OSError) as failure:
                    qwen.serve(Path(temporary), port, platform="linux")
                self.assertEqual(failure.exception.errno, errno.EADDRINUSE)
                spawn.assert_not_called()

    def test_server_closed_connection_allows_immediate_port_preflight(self):
        for option in (socket.SO_REUSEADDR, socket.SO_REUSEPORT):
            with self.subTest(option=option), socket.socket() as listener:
                listener.setsockopt(socket.SOL_SOCKET, option, 1)
                listener.bind(("127.0.0.1", 0))
                listener.listen()
                port = listener.getsockname()[1]
                with socket.create_connection(("127.0.0.1", port), timeout=2) as client:
                    connection, _ = listener.accept()
                    with connection:
                        connection.shutdown(socket.SHUT_WR)
                        self.assertEqual(client.recv(1), b"")
                        client.shutdown(socket.SHUT_WR)
                        self.assertEqual(connection.recv(1), b"")
                listener.close()
                with tempfile.TemporaryDirectory() as temporary:
                    with (
                        patch.object(qwen, "available_memory", return_value=20 * 1024**3),
                        patch.object(
                            qwen, "verify", side_effect=ValueError("reached model check")
                        ) as verify,
                        patch.object(qwen, "spawn") as spawn,
                        redirect_stdout(io.StringIO()),
                    ):
                        with self.assertRaisesRegex(ValueError, "reached model check"):
                            qwen.serve(Path(temporary), port, platform="linux")
                        verify.assert_called_once()
                        spawn.assert_not_called()

    def test_owned_fake_server_reports_ready_then_exits_without_model_inference(self):
        script = """
import json,sys,threading
from http.server import HTTPServer,BaseHTTPRequestHandler
class Handler(BaseHTTPRequestHandler):
    def log_message(self,*args):pass
    def do_GET(self):
        body=b'{"status":"ok"}'
        self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers()
        self.wfile.write(body)
        threading.Timer(.1,self.server.shutdown).start()
server=HTTPServer(('127.0.0.1',int(sys.argv[1])),Handler)
server.serve_forever(poll_interval=.01)
server.server_close()
"""
        with socket.socket() as reserve:
            reserve.bind(("127.0.0.1", 0))
            port = reserve.getsockname()[1]
        with tempfile.TemporaryDirectory() as temporary:
            with (
                patch.object(qwen, "available_memory", return_value=20 * 1024**3),
                patch.object(qwen, "verify"),
                patch.object(qwen, "executable", return_value=Path(sys.executable)),
                patch.object(
                    qwen, "command", return_value=[sys.executable, "-c", script, str(port)]
                ),
                redirect_stdout(io.StringIO()) as output,
            ):
                qwen.serve(Path(temporary), port, platform="linux")
            rows = [json.loads(line) for line in output.getvalue().splitlines()]
            self.assertTrue(rows[-1]["ready"])
            self.assertEqual(rows[-1]["port"], port)

    def test_cpu_and_metal_argv_keep_embedding_contract(self):
        for platform, device, layers in (("linux", "none", "0"), ("darwin", "MTL0", "all")):
            args = qwen.command(Path("server"), Path("model"), 8081, platform)
            slot_tokens = 2048 if platform == "linux" else 8192
            self.assertEqual(args[args.index("--device") + 1], device)
            self.assertEqual(args[args.index("--n-gpu-layers") + 1], layers)
            for name, expected in (
                ("--pooling", "last"),
                ("--parallel", "4"),
                ("--ctx-size", str(4 * slot_tokens)),
                ("--batch-size", str(slot_tokens)),
                ("--ubatch-size", str(slot_tokens)),
                ("--flash-attn", "on"),
                ("--host", "127.0.0.1"),
            ):
                self.assertEqual(args[args.index(name) + 1], expected)

    def test_cpu_memory_refusal_precedes_model_verification(self):
        with (
            tempfile.TemporaryDirectory() as temporary,
            patch.object(qwen.sys, "platform", "linux"),
        ):
            with (
                patch.object(qwen, "available_memory", return_value=8 * 1024**3),
                patch.object(qwen, "verify") as verify,
            ):
                with self.assertRaisesRegex(ValueError, "16 GiB"):
                    qwen.serve(Path(temporary), 8081, platform="linux")
                verify.assert_not_called()

    def test_cgroup_limits_and_usage_reduce_host_available_memory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            proc, group = root / "proc", root / "cgroup"
            (proc / "self").mkdir(parents=True)
            (group / "slice/leaf").mkdir(parents=True)
            atomic_write_text(group / "cgroup.controllers", "memory cpu\n")
            atomic_write_text(proc / "self/mountinfo",
                              f"1 0 0:1 / {group} rw - cgroup2 cgroup rw\n")
            (proc / "meminfo").write_text("MemAvailable: 33554432 kB\n")
            (proc / "self/cgroup").write_text("0::/slice/leaf\n")
            (group / "memory.max").write_text("max\n")
            (group / "slice/memory.max").write_text(str(20 * 1024**3))
            (group / "slice/memory.current").write_text(str(5 * 1024**3))
            atomic_write_text(group / "slice/memory.stat", "active_file 0\ninactive_file 0\n")
            self.assertEqual(qwen.available_memory(proc), 15 * 1024**3)
