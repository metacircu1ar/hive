"""Private PTY endpoints with an explicit, shared idempotent capture retirement."""

from contextlib import contextmanager
import os
import pty
from .progress import cleanup_all, finalizer


@contextmanager
def terminal():
    master, slave = pty.openpty()
    capture = master

    def retire_capture():
        nonlocal capture
        if capture is not None:
            descriptor, capture = capture, None
            os.close(descriptor)

    with finalizer(lambda: cleanup_all(retire_capture, lambda: os.close(slave))):
        yield master, slave, retire_capture
