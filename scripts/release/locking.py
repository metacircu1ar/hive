"""Apply the maintainer command's cancellation/deadline policy to file locks."""

from contextlib import contextmanager
from . import files
from .cancellation import bounded, check, pause


@contextmanager
def directory_lock(path):
    with files.directory_lock(path, check=check, pause=pause, timeout=bounded(30)):
        yield
