"""Interrupt long native SQLite statements without losing the cancellation cause."""

from contextlib import contextmanager
import sqlite3
from release.cancellation import Cancelled, WorkerStopped, check


@contextmanager
def cancellable(connection):
    pending = []

    def progress():
        try:
            check()
        except (Cancelled, WorkerStopped) as error:
            pending.append(error)
            return 1
        return 0

    connection.set_progress_handler(progress, 1000)
    try:
        yield connection
    except sqlite3.OperationalError:
        if pending:
            raise pending[0] from None
        raise
    finally:
        connection.set_progress_handler(None, 0)
