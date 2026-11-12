"""Private temporary roots with pinned identity and primary-preserving cleanup."""

from contextlib import contextmanager
import os
from pathlib import Path
import shutil
import tempfile
from .cancellation import check
from .files import _owned_fd
from .progress import finalizer


@contextmanager
def directory(*, prefix, parent=None):
    check()
    path = Path(tempfile.mkdtemp(prefix=prefix, dir=parent)).resolve()
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    except OSError as error:
        error.retained_root = str(path)
        raise
    with _owned_fd(descriptor) as fd:
        original = os.fstat(fd)
        retained = False

        def remove():
            current = path.lstat()
            if (current.st_dev, current.st_ino) != (original.st_dev, original.st_ino):
                raise RuntimeError("Temporary root changed; retaining foreign replacement")
            shutil.rmtree(path)

        with finalizer(lambda: None if retained else remove()):
            try:
                check()
                yield path
            except BaseException as error:
                if getattr(error, "owned_cleanup_uncertain", False):
                    retained = True
                    error.retained_root = str(path)
                raise
