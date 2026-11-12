"""Bounded child IO and group cleanup while the unreaped leader pins its identity."""

from contextlib import contextmanager
import errno
import os
import signal
import subprocess
import time
from .cancellation import bounded, check, cleanup, pause, scope
from .child_observer import Observer, OwnershipLost, only_leader
from .progress import cleanup_all, finalizer


class Owned:
    def __init__(
        self,
        command,
        *,
        env=None,
        cwd=None,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        limit=4 * 1024 * 1024,
        stop_signal=signal.SIGTERM,
        stop_grace=0.5,
        pass_fds=(),
        capture_fd=None,
        capture_close=None,
    ):
        check()
        if not isinstance(pass_fds, tuple) or any(type(fd) is not int or fd < 3 for fd in pass_fds):
            raise ValueError("Inherited descriptors must be an explicit tuple of non-stdio fds")
        for fd in pass_fds:
            os.fstat(fd)
        if capture_fd is not None and capture_close is None:
            raise ValueError("Borrowed capture requires its owner's retirement callback")
        self.command, self.limit, self.stop_signal = command, limit, stop_signal
        self.stop_grace, self.kill_confirmed, self.output_overflow = stop_grace, False, False
        self.capture_fd, self.capture_eof = capture_fd, False
        self.capture_close = capture_close
        self.observer, self.completed, self.lost = None, False, False
        self.output, self.errors, self.input = bytearray(), bytearray(), bytearray()
        self.counts, self.files, self.close_input = {}, {}, False
        self.nonblocking, self.capture_errors = set(), []
        self.observer = Observer()
        try:
            self.process = subprocess.Popen(
                command,
                env=env,
                cwd=cwd,
                stdin=stdin,
                stdout=stdout,
                stderr=stderr,
                start_new_session=True,
                pass_fds=pass_fds,
            )
        except BaseException:
            with finalizer(self.observer.close):
                raise
        self.pid = self.process.pid
        try:
            self.observer.attach(self.pid)
            for name in ("stdin", "stdout", "stderr"):
                stream = getattr(self.process, name)
                if stream is not None:
                    os.set_blocking(stream.fileno(), False)
                    self.nonblocking.add(stream.fileno())
            if capture_fd is not None:
                os.set_blocking(capture_fd, False)
                self.nonblocking.add(capture_fd)
            for stream in (stdout, stderr):
                if hasattr(stream, "fileno"):
                    fd = stream.fileno()
                    self.files[fd] = os.fstat(fd).st_size
        except BaseException:
            with finalizer(self.close):
                with finalizer(self.retire_unusable):
                    raise

    @property
    def returncode(self):
        return self.process.returncode

    def send(self, data, *, close=False):
        check()
        self.counts["stdin"] = self.counts.get("stdin", 0) + len(data)
        if self.counts["stdin"] > self.limit:
            raise ValueError("Owned process input exceeded its bound")
        self.input.extend(data)
        self.close_input = close

    def pump(self, *, draining=False):
        check()
        readers = []
        for name, target in (("stdout", self.output), ("stderr", self.errors)):
            stream = getattr(self.process, name)
            if stream is not None and not stream.closed and stream.fileno() in self.nonblocking:
                readers.append((name, stream.fileno(), target))
        if self.capture_fd in self.nonblocking and not self.capture_eof:
            readers.append(("terminal", self.capture_fd, self.output))
        for name, descriptor, target in readers:
            for _ in range(8):
                try:
                    data = os.read(descriptor, 65536)
                except BlockingIOError:
                    break
                except OSError as error:
                    if name == "terminal" and error.errno == errno.EIO:
                        self.capture_eof = True
                    else:
                        self.capture_failed(descriptor, error, draining)
                    break
                if not data:
                    break
                self.counts[name] = self.counts.get(name, 0) + len(data)
                if self.counts[name] > self.limit:
                    self.output_overflow = True
                    if not draining:
                        raise ValueError("Owned process output exceeded its bound")
                target.extend(data[: max(0, self.limit - len(target))])
        if not draining:
            self._input()
        for fd, original in list(self.files.items()):
            try:
                size = os.fstat(fd).st_size - original
            except OSError as error:
                self.capture_failed(fd, error, draining)
                continue
            if size > self.limit:
                self.output_overflow = True
                if not draining:
                    raise ValueError("Owned process log exceeded its bound")

    def capture_failed(self, descriptor, error, draining):
        self.nonblocking.discard(descriptor)
        self.files.pop(descriptor, None)
        try:
            self.retire_capture(descriptor)
        except OSError as close_error:
            error.cleanup_errors = getattr(error, "cleanup_errors", ()) + (close_error,)
        if not draining:
            raise error
        self.capture_errors.append(error)

    def retire_capture(self, descriptor):
        for name in ("stdout", "stderr"):
            stream = getattr(self.process, name)
            if stream is not None and not stream.closed and stream.fileno() == descriptor:
                stream.close()
        if descriptor == self.capture_fd and not self.capture_eof:
            self.capture_eof = True
            self.capture_close()

    def retire_unusable(self):
        descriptors = [
            stream.fileno()
            for name in ("stdout", "stderr")
            if (stream := getattr(self.process, name)) is not None and not stream.closed
        ]
        if self.capture_fd is not None:
            descriptors.append(self.capture_fd)
        cleanup_all(
            *(
                lambda fd=fd: self.retire_capture(fd)
                for fd in descriptors
                if fd not in self.nonblocking
            )
        )

    def capture_result(self):
        if self.output_overflow:
            self.capture_errors.append(ValueError("Owned process output exceeded its bound"))
        if self.capture_errors:
            error, *later = self.capture_errors
            error.cleanup_errors = getattr(error, "cleanup_errors", ()) + tuple(later)
            raise error

    def _input(self):
        stream = self.process.stdin
        if stream is None or stream.closed:
            return
        if self.input:
            try:
                written = os.write(stream.fileno(), self.input[:65536])
            except BlockingIOError:
                return
            except BrokenPipeError:
                stream.close()
                self.input.clear()
                return
            del self.input[:written]
        if self.close_input and not self.input:
            stream.close()

    def exited(self):
        if self.completed:
            return True
        self.pump()
        return self.observer.ready()

    def line(self, *, timeout=30):
        deadline = time.monotonic() + bounded(timeout)
        while True:
            self.pump()
            if b"\n" in self.output:
                at = self.output.index(b"\n") + 1
                line = bytes(self.output[:at])
                del self.output[:at]
                return line
            if self.observer.ready():
                self.pump()
                if b"\n" in self.output:
                    continue
                raise RuntimeError("Owned process exited before its reply")
            if time.monotonic() >= deadline:
                raise subprocess.TimeoutExpired(self.command, timeout)
            pause(0.02)

    def wait(self, *, timeout):
        if self.completed:
            return self.returncode
        deadline = time.monotonic() + bounded(timeout)
        while not self.exited():
            if time.monotonic() >= deadline:
                raise subprocess.TimeoutExpired(
                    self.command, timeout, output=bytes(self.output), stderr=bytes(self.errors)
                )
            pause(0.02)
        with cleanup():
            self.force()
            self.pump(draining=True)
            self.capture_result()
        return self.returncode

    def _signal(self, number):
        if self.lost or self.observer is not None and self.observer.lost:
            self._lost()
            raise OwnershipLost("Owned child status was lost; signals are forbidden")
        if self.completed:
            return
        if self.observer is not None:
            self.observer.owned()
        try:
            os.killpg(self.pid, number)
        except ProcessLookupError:
            # Only while the leader is still owned and unreaped.
            pass
        except PermissionError:
            if self.observer is None or not self.observer.ready() or not only_leader(self.pid):
                raise

    def _lost(self):
        self.lost = True
        # Disable Popen's implicit destructor poll, without inventing an exit status.
        self.process._child_created = False

    def _reap(self):
        if self.completed:
            return
        if self.lost or self.observer is not None and self.observer.lost:
            self._lost()
            raise OwnershipLost("Owned child status was lost; reaping is forbidden")
        deadline = time.monotonic() + 2
        while True:
            try:
                pid, status = os.waitpid(self.pid, os.WNOHANG)
            except ChildProcessError as error:
                self._lost()
                raise OwnershipLost("Owned child status was lost during final reap") from error
            if pid == self.pid:
                self.process.returncode = os.waitstatus_to_exitcode(status)
                self.completed = True
                return
            if time.monotonic() >= deadline:
                self._lost()
                raise RuntimeError("Owned child did not reap before its cleanup deadline")
            pause(0.02)

    def stop(self):
        with cleanup():
            if self.completed:
                return
            with finalizer(self.force):
                self._signal(self.stop_signal)
                deadline = time.monotonic() + self.stop_grace
                while True:
                    self.pump(draining=True)
                    if (
                        self.observer is None
                        or self.observer.ready()
                        or time.monotonic() >= deadline
                    ):
                        break
                    pause(0.02)
            self.pump(draining=True)
            self.capture_result()

    def force(self):
        def signal_group():
            self._signal(signal.SIGKILL)
            self.kill_confirmed = True

        try:
            cleanup_all(signal_group, self._reap)
        except BaseException as error:
            if not self.completed or not self.kill_confirmed:
                error.owned_cleanup_uncertain = True
            raise

    def close(self):
        with cleanup():
            actions = [self.stop]
            actions += [
                stream.close
                for name in ("stdin", "stdout", "stderr")
                if (stream := getattr(self.process, name)) is not None and not stream.closed
            ]
            if self.observer is not None:
                actions.append(self.observer.close)
            try:
                cleanup_all(*actions)
            except BaseException as error:
                if not self.completed or not self.kill_confirmed:
                    error.owned_cleanup_uncertain = True
                    self._lost()
                raise


@contextmanager
def spawn(command, **options):
    with scope():
        process = Owned(command, **options)
        with finalizer(process.close):
            check()
            yield process
