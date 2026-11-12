"""Cooperative signal cancellation at explicit, ownership-safe boundaries."""

from contextlib import contextmanager
from contextvars import ContextVar, copy_context
import ctypes
import os
import signal
import time

_active = ContextVar("release_cancellation", default=None)
_cleaning = ContextVar("release_cleanup", default=False)
_deadline = ContextVar("release_deadline", default=None)
_worker_stop = ContextVar("release_worker_stop", default=None)
_SIGNALS = (signal.SIGINT, signal.SIGTERM, signal.SIGHUP, signal.SIGCHLD)


class Cancelled(BaseException):
    def __init__(self, number):
        super().__init__("Maintainer command cancelled")
        self.number = number


class WorkerStopped(BaseException):
    """A main-thread owner requested cooperative worker shutdown."""


def worker_context(stop):
    """Share main-thread cancellation without installing signal handlers in a thread."""
    if _active.get() is None:
        raise RuntimeError("Workers require a main-thread cancellation scope")
    context = copy_context()
    context.run(_worker_stop.set, stop)
    return context


def _action(number, previous=None):
    libc = ctypes.CDLL(None, use_errno=True)
    libc.sigaction.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p]
    libc.sigaction.restype = ctypes.c_int
    storage = (ctypes.c_long * 64)()
    result = libc.sigaction(
        number,
        ctypes.byref(previous) if previous is not None else None,
        ctypes.byref(storage) if previous is None else None,
    )
    if result != 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code))
    return storage


class Scope:
    def __init__(self):
        self.first, self.finishing, self.saved = None, False, []
        self.defer, self.completion = False, None

    def record(self, number, _frame):
        if self.first is None:
            self.first = (number, self.finishing)

    def __enter__(self):
        from .progress import finalizer

        self.token = _active.set(self)
        try:
            for number in _SIGNALS:
                self.saved.append((number, signal.getsignal(number), _action(number)))
                signal.signal(number, signal.SIG_DFL if number == signal.SIGCHLD else self.record)
        except BaseException:
            with finalizer(self.restore):
                raise
        return self

    def restore(self):
        from .progress import cleanup_all, finalizer

        def one(number, handler, action):
            with finalizer(lambda: _action(number, action)):
                signal.signal(number, handler)

        try:
            cleanup_all(*(lambda saved=saved: one(*saved) for saved in reversed(self.saved)))
        finally:
            _active.reset(self.token)

    def __exit__(self, kind, error, traceback):
        from .progress import finalizer

        with finalizer(self.restore):
            if error is not None:
                raise error.with_traceback(traceback)


@contextmanager
def scope():
    if _active.get() is not None:
        yield _active.get()
    else:
        with Scope() as state:
            yield state


@contextmanager
def cleanup():
    token = _cleaning.set(True)
    try:
        yield
    finally:
        _cleaning.reset(token)


def check():
    state = _active.get()
    if (
        not _cleaning.get()
        and state is not None
        and not state.finishing
        and state.first is not None
        and not state.first[1]
    ):
        raise Cancelled(state.first[0])
    stop = _worker_stop.get()
    if not _cleaning.get() and stop is not None and stop.is_set():
        raise WorkerStopped()


def pause(seconds):
    deadline = time.monotonic() + max(0, seconds)
    while True:
        check()
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return
        time.sleep(min(0.1, remaining))


@contextmanager
def deadline(seconds):
    until = time.monotonic() + seconds
    previous = _deadline.get()
    token = _deadline.set(min(until, previous) if previous is not None else until)
    try:
        yield
    finally:
        _deadline.reset(token)


def bounded(seconds):
    until = _deadline.get()
    return (
        min(seconds, max(0, until - time.monotonic()))
        if until is not None and not _cleaning.get()
        else seconds
    )


def finish(action):
    """Choose success after cleanup; later signals cannot revise that decision."""
    state = _active.get()
    if state is not None:
        if state.defer:
            if state.completion is not None:
                raise RuntimeError("A command may declare only one final completion")
            state.completion = action
            return None
        state.finishing = True
        if state.first is not None and not state.first[1]:
            state.finishing = False
            raise Cancelled(state.first[0])
    return action()
