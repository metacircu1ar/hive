"""Bounded private native stderr, separate from redacted structured diagnostics."""

from contextlib import contextmanager
from contextvars import ContextVar
import os
from pathlib import Path
import secrets
from . import files

_current = ContextVar("release_native_evidence", default=None)


class Evidence:
    def __init__(self, path, stream, limit):
        self.path, self.stream, self.limit, self.size = path, stream, limit, 0

    def retain(self, data):
        if not data:
            return
        before, named = os.fstat(self.stream.fileno()), self.path.lstat()
        if (before.st_dev, before.st_ino) != (named.st_dev, named.st_ino):
            raise RuntimeError("Native stderr evidence changed; refusing a rebound path")
        if self.size + len(data) > self.limit:
            raise ValueError("Native stderr evidence exceeded its byte budget")
        offset = 0
        while offset < len(data):
            written = self.stream.write(memoryview(data)[offset : offset + 1024 * 1024])
            if not written:
                raise OSError("Incomplete native stderr evidence write")
            offset += written
            self.size += written


def current():
    return _current.get()


@contextmanager
def detached():
    """Worker processes retain their own result, never share another thread's stream."""
    token = _current.set(None)
    try:
        yield
    finally:
        _current.reset(token)


@contextmanager
def evidence(directory, *, limit=64 * 1024 * 1024):
    directory = Path(directory)
    files.create_directory(directory, exist_ok=True)
    path = directory / ("native-" + secrets.token_hex(12) + ".stderr")
    files.atomic_write_bytes(path, b"", mode=0o600, create_once=True)
    with files.raw_log(path) as stream:
        sink = Evidence(path, stream, limit)
        token = _current.set(sink)
        try:
            yield sink
        except BaseException as error:
            if sink.size:
                error.native_stderr = str(path)
            raise
        finally:
            _current.reset(token)
