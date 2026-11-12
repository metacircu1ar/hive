"""Native ownership, descendant retirement, bounds and cooperative signal faults."""

import os
from pathlib import Path
import signal
import sys
import tempfile
import time
import unittest
from unittest.mock import patch
from . import cancellation, files
from .child_observer import OwnershipLost
from .owned_process import Owned, spawn
from .process import run
from .progress import entry, finalizer


class OwnedProcessTests(unittest.TestCase):
    def test_cancelled_child_retires_its_inherited_lock_before_next_acquisition(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "publication"
            with self.assertRaises(cancellation.Cancelled):
                with files.record_lock(path) as lock, cancellation.Scope() as state:
                    with spawn(
                        [
                            sys.executable,
                            "-c",
                            "import time;print('ready',flush=True);time.sleep(60)",
                        ],
                        pass_fds=(lock,),
                    ) as child:
                        self.assertEqual(child.line(), b"ready\n")
                        pid = child.pid
                        state.record(signal.SIGTERM, None)
                        child.wait(timeout=10)
            self.absent(pid)
            with files.record_lock(
                path, check=cancellation.check, pause=cancellation.pause, timeout=0.1
            ):
                pass

    def test_only_explicitly_admitted_descriptor_is_inherited_and_parent_keeps_ownership(self):
        with tempfile.TemporaryDirectory() as directory:
            first = os.open(Path(directory) / "first", os.O_RDWR | os.O_CREAT, 0o600)
            second = os.open(Path(directory) / "second", os.O_RDWR | os.O_CREAT, 0o600)
            try:
                code = """
import os,sys
os.write(int(sys.argv[1]),b'child')
try:
    os.fstat(int(sys.argv[2]))
except OSError:
    pass
else:
    raise RuntimeError('Unadmitted descriptor inherited')
"""
                result = run(
                    [sys.executable, "-c", code, str(first), str(second)],
                    pass_fds=(first,),
                    check=True,
                )
                self.assertEqual(result.returncode, 0)
                self.assertEqual(os.pread(first, 10, 0), b"child")
                self.assertFalse(os.get_inheritable(first))
                os.fstat(second)
            finally:
                os.close(first)
                os.close(second)

    def test_unconfirmed_child_holder_keeps_lock_when_parent_scope_unwinds(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "publication"
            owner = None
            try:
                error = RuntimeError("synthetic unconfirmed cleanup")
                error.owned_cleanup_uncertain = True
                with self.assertRaisesRegex(TimeoutError, "primary operation") as raised:
                    with files.record_lock(path) as lock:
                        owner = Owned(
                            [
                                sys.executable,
                                "-c",
                                "import time; print('ready',flush=True);time.sleep(60)",
                            ],
                            pass_fds=(lock,),
                        )
                        self.assertEqual(owner.line(), b"ready\n")
                        with (
                            patch.object(owner, "close", side_effect=error),
                            finalizer(owner.close),
                        ):
                            raise TimeoutError("synthetic primary operation failure")
                self.assertTrue(raised.exception.owned_cleanup_uncertain)
                self.assertEqual(raised.exception.cleanup_errors, (error,))
                with self.assertRaises(TimeoutError):
                    with files.record_lock(
                        path, check=cancellation.check, pause=cancellation.pause, timeout=0.1
                    ):
                        self.fail("A second downloader acquired the still-held lock")
                owner.close()
                owner = None
                with files.record_lock(
                    path, check=cancellation.check, pause=cancellation.pause, timeout=0.1
                ):
                    pass
            finally:
                if owner is not None:
                    owner.close()

    def test_failed_capture_adoption_never_reads_a_blocking_pipe(self):
        failure, blocked = OSError("synthetic capture adoption failure"), []
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / "ready"
            code = (
                "import signal,sys,time;from pathlib import Path;"
                "signal.signal(signal.SIGTERM,signal.SIG_IGN);"
                "Path(sys.argv[1]).write_text('ready');time.sleep(30)"
            )
            child = Owned.__new__(Owned)
            original = os.read

            def fail_adoption(*args):
                until = time.monotonic() + 2
                while not marker.exists() and time.monotonic() < until:
                    time.sleep(0.01)
                self.assertTrue(marker.exists())
                raise failure

            def checked_read(descriptor, length):
                process = getattr(child, "process", None)
                if (
                    process is not None
                    and descriptor in {process.stdout.fileno(), process.stderr.fileno()}
                    and os.get_blocking(descriptor)
                ):
                    blocked.append(descriptor)
                    raise OSError("refuse test's blocking capture read")
                return original(descriptor, length)

            began = time.monotonic()
            with (
                cancellation.scope(),
                patch("release.owned_process.os.set_blocking", side_effect=fail_adoption),
                patch("release.owned_process.os.read", side_effect=checked_read),
                self.assertRaises(OSError) as raised,
            ):
                child.__init__([sys.executable, "-c", code, str(marker)])
            self.assertIs(raised.exception, failure)
            self.assertEqual(blocked, [])
            self.assertTrue(child.process.stdout.closed)
            self.assertTrue(child.process.stderr.closed)
            self.assertTrue(child.completed)
            self.assertLess(time.monotonic() - began, 4)
            self.absent(child.pid)

    def test_failed_capture_closes_pipe_so_owner_can_retire_a_separate_child(self):
        code = """
import os,sys
from release.owned_process import spawn
from release.progress import entry,finalizer
from release.cancellation import pause
def flush():
    try:os.write(1,b'x'*262144)
    except BrokenPipeError:pass
    pause(.7)
def work():
    with spawn([sys.executable,'-c','import time;time.sleep(30)']) as worker:
        with finalizer(flush):
            print(worker.pid,flush=True)
            worker.wait(timeout=30)
sys.exit(entry(work,name='verify'))
"""
        failure = OSError("synthetic capture read failure")
        with (
            self.assertRaises(OSError) as raised,
            spawn(
                [sys.executable, "-c", code],
                env=dict(os.environ, PYTHONPATH="scripts"),
                stop_grace=3,
            ) as child,
        ):
            worker = int(child.line(timeout=2))
            descriptor, original = child.process.stdout.fileno(), os.read

            def read(fd, length):
                if fd == descriptor:
                    raise failure
                return original(fd, length)

            with patch("release.owned_process.os.read", side_effect=read):
                child.stop()
        self.assertIs(raised.exception, failure)
        self.assertEqual(child.returncode, 143)
        self.assertTrue(child.completed)
        self.assertFalse(getattr(failure, "owned_cleanup_uncertain", False))
        self.absent(worker)

    def test_failed_borrowed_terminal_capture_retires_through_its_owner(self):
        from .terminal import terminal

        with terminal() as (master, slave, retire):
            with (
                self.assertRaises(OSError) as raised,
                spawn(
                    [sys.executable, "-c", "import time;time.sleep(30)"],
                    stdin=slave,
                    stdout=slave,
                    stderr=slave,
                    capture_fd=master,
                    capture_close=retire,
                ) as child,
            ):
                failure, original = OSError("synthetic terminal capture failure"), os.read

                def read(fd, length):
                    if fd == master:
                        raise failure
                    return original(fd, length)

                with patch("release.owned_process.os.read", side_effect=read):
                    child.stop()
            self.assertIs(raised.exception, failure)
            with self.assertRaises(OSError):
                os.fstat(master)
            self.assertTrue(child.completed)

    def test_unconfirmed_group_signal_retains_uncertainty_even_when_leader_reaps(self):
        failure = OSError("synthetic group completion uncertainty")
        with self.assertRaises(OSError) as raised, spawn([sys.executable, "-c", "pass"]) as child:
            original = child._signal

            def uncertain(number):
                original(number)
                if number == signal.SIGKILL:
                    raise failure

            with patch.object(child, "_signal", uncertain):
                child.wait(timeout=2)
        self.assertIs(raised.exception, failure)
        self.assertTrue(child.completed)
        self.assertTrue(raised.exception.owned_cleanup_uncertain)
        self.absent(child.pid)

    def test_cleanup_drains_output_and_respects_a_callers_longer_grace(self):
        code = """
import os,signal,sys,time
def retire(*args):
    os.write(1,b'x'*262144)
    time.sleep(.7)
    sys.exit(0)
signal.signal(signal.SIGTERM,retire)
print('ready',flush=True)
while True:time.sleep(.1)
"""
        with spawn([sys.executable, "-c", code], stop_grace=2) as child:
            self.assertEqual(child.line(timeout=2), b"ready\n")
        self.assertEqual(child.returncode, 0)
        self.assertEqual(len(child.output), 262144)

    def test_observation_failure_still_forces_and_reaps_the_ignoring_child(self):
        failure = OSError("synthetic exit observer failure")
        code = (
            "import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);"
            "print('ready',flush=True);time.sleep(30)"
        )
        with self.assertRaises(OSError) as raised, spawn([sys.executable, "-c", code]) as child:
            self.assertEqual(child.line(timeout=2), b"ready\n")
            with patch.object(child.observer, "ready", side_effect=failure):
                child.stop()
        self.assertIs(raised.exception, failure)
        self.assertTrue(child.completed)
        self.assertTrue(child.kill_confirmed)
        self.assertFalse(getattr(raised.exception, "owned_cleanup_uncertain", False))
        self.absent(child.pid)

    def test_final_reply_written_between_pump_and_exit_observation_is_not_lost(self):
        with spawn([sys.executable, "-c", "input();print('reply',flush=True)"], stdin=-1) as child:
            observe = child.observer.ready

            def exit_after_write(*args):
                child.send(b"go\n", close=True)
                child._input()
                until = time.monotonic() + 2
                while not observe() and time.monotonic() < until:
                    time.sleep(0.01)
                self.assertTrue(observe())
                return True

            with patch.object(child.observer, "ready", exit_after_write):
                self.assertEqual(child.line(timeout=2), b"reply\n")
            self.assertEqual(child.wait(timeout=2), 0)

    def test_cleanup_cannot_turn_systemexit_zero_into_success(self):
        def retire():
            raise SystemExit(0)

        def work():
            with finalizer(retire):
                pass

        self.assertEqual(entry(work, name="verify"), 1)

    def test_observer_failure_happens_before_spawn_and_io_adoption_failure_reaps(self):
        with (
            patch("release.owned_process.Observer", side_effect=OSError("observer failure")),
            patch("release.owned_process.subprocess.Popen") as create,
        ):
            with self.assertRaises(OSError):
                with spawn([sys.executable, "-c", "pass"]):
                    self.fail("Observation failed before ownership")
            create.assert_not_called()
        child = Owned.__new__(Owned)
        with (
            cancellation.scope(),
            patch(
                "release.owned_process.os.set_blocking", side_effect=OSError("IO adoption failure")
            ),
        ):
            with self.assertRaises(OSError):
                child.__init__([sys.executable, "-c", "import time;time.sleep(30)"])
        self.assertTrue(child.completed)
        self.assertIsNotNone(child.returncode)

    def test_native_exit_io_and_output_bound(self):
        result = run(
            [sys.executable, "-c", "import sys; print(sys.stdin.read());sys.exit(17)"],
            input="request",
            timeout=2,
            text=True,
        )
        self.assertEqual((result.returncode, result.stdout), (17, "request\n"))
        with self.assertRaisesRegex(ValueError, "output exceeded"):
            run([sys.executable, "-c", "import os;os.write(1,b'x'*65536)"], limit=1024, timeout=2)
        with (
            self.assertRaisesRegex(ValueError, "output exceeded"),
            spawn([sys.executable, "-c", "import os;os.write(1,b'x'*65536)"], limit=1024) as child,
        ):
            child.wait(timeout=2)
        self.assertTrue(child.completed)

    def test_immediate_exit_still_retires_a_pipe_holding_descendant(self):
        code = (
            "import subprocess,sys;"
            "subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)'])"
        )
        began = time.monotonic()
        with spawn([sys.executable, "-c", code]) as child:
            group = child.pid
            self.assertEqual(child.wait(timeout=2), 0)
        self.assertLess(time.monotonic() - began, 3)
        self.absent(group)

    def absent(self, group):
        deadline = time.monotonic() + 1
        while time.monotonic() < deadline:
            try:
                os.killpg(group, 0)
            except ProcessLookupError:
                return
            except PermissionError:
                pass
            time.sleep(0.02)
        self.fail("Owned process group survived cleanup")

    def test_actual_lost_status_forbids_every_later_signal(self):
        with (
            self.assertRaises(OwnershipLost),
            spawn([sys.executable, "-c", "import time;time.sleep(30)"]) as child,
        ):
            os.kill(child.pid, signal.SIGKILL)
            os.waitpid(child.pid, 0)
            with patch("os.killpg", side_effect=AssertionError("signal after ownership loss")):
                try:
                    child.wait(timeout=1)
                except OwnershipLost:
                    self.assertTrue(child.observer.lost)
                    child.close()

    def test_ignored_sigchld_is_reset_before_spawn_and_restored(self):
        code = """
import signal,sys
from release.process import run
signal.signal(signal.SIGCHLD,signal.SIG_IGN)
result=run([sys.executable,'-c','import sys;sys.exit(17)'],timeout=2)
assert result.returncode==17
assert signal.getsignal(signal.SIGCHLD)==signal.SIG_IGN
"""
        result = run(
            [sys.executable, "-c", code], env=dict(os.environ, PYTHONPATH="scripts"), timeout=4
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_signal_retires_nested_owned_group_and_does_not_publish_success(self):
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / "ready"
            report = Path(directory) / "report"
            code = """
import os,sys
from pathlib import Path
from release.owned_process import spawn
from release.cancellation import pause,finish
from release.progress import entry
def work():
    code=("import os,signal,time,subprocess,sys;from pathlib import Path;"
          "signal.signal(signal.SIGTERM,signal.SIG_IGN);"
          "subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)']);"
          "Path(sys.argv[1]).write_text(str(os.getpgrp()));time.sleep(30)")
    with spawn([sys.executable,'-c',code,sys.argv[1]]) as child:
        child.wait(timeout=300)
    finish(lambda: Path(sys.argv[2]).write_text('passed'))
sys.exit(entry(work,name='verify'))
"""
            began = time.monotonic()
            with spawn(
                [sys.executable, "-c", code, str(marker), str(report)],
                env=dict(os.environ, PYTHONPATH="scripts"),
            ) as driver:
                deadline = time.monotonic() + 2
                while not marker.exists() and time.monotonic() < deadline:
                    cancellation.pause(0.02)
                self.assertTrue(marker.exists())
                group = int(marker.read_text())
                driver._signal(signal.SIGTERM)
                self.assertEqual(driver.wait(timeout=4), 143)
            self.assertLess(time.monotonic() - began, 5)
            self.assertFalse(report.exists())
            self.absent(group)

    def test_repeated_signals_at_finalizer_entry_cannot_skip_cleanup(self):
        seen = []

        def work():
            with finalizer(lambda: (seen.append("cleanup"), os.kill(os.getpid(), signal.SIGHUP))):
                os.kill(os.getpid(), signal.SIGTERM)
            cancellation.finish(lambda: seen.append("passed"))

        self.assertEqual(entry(work, name="verify"), 143)
        self.assertEqual(seen, ["cleanup"])

    def test_signal_after_success_decision_cannot_reverse_a_committed_result(self):
        seen = []

        def work():
            cancellation.finish(
                lambda: (os.kill(os.getpid(), signal.SIGTERM), seen.append("passed"))
            )

        self.assertEqual(entry(work, name="verify"), 0)
        self.assertEqual(seen, ["passed"])

    def test_signal_after_rename_waits_for_publication_outcome(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "record"
            original = files.os.replace

            def renamed(*args, **kwargs):
                result = original(*args, **kwargs)
                os.kill(os.getpid(), signal.SIGTERM)
                return result

            with cancellation.scope(), patch.object(files.os, "replace", renamed):
                files.atomic_write_bytes(path, b"committed")
                self.assertEqual(path.read_bytes(), b"committed")
                with self.assertRaises(cancellation.Cancelled):
                    cancellation.check()
            self.assertEqual(list(Path(directory).iterdir()), [path])

    def test_signal_after_temporary_creation_cannot_skip_descriptor_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "record"
            original = files._Preparation.prepare
            descriptors = []

            def prepared(preparation, mode):
                original(preparation, mode)
                descriptors.append(preparation.fd)
                os.kill(os.getpid(), signal.SIGTERM)

            with cancellation.scope(), patch.object(files._Preparation, "prepare", prepared):
                files.atomic_write_bytes(path, b"committed")
                with self.assertRaises(cancellation.Cancelled):
                    cancellation.check()
            self.assertEqual(path.read_bytes(), b"committed")
            with self.assertRaises(OSError):
                os.fstat(descriptors[0])
            self.assertEqual(list(Path(directory).iterdir()), [path])
