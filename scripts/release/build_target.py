"""Exclusively own a private Cargo namespace through artifact freeze."""

from contextlib import contextmanager
import os
from pathlib import Path
import shutil
import stat
import tempfile
from .files import _owned_fd, sync_directory
from .locking import directory_lock
from .progress import finalizer


def admit(path):
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid() or info.st_mode & 0o077:
        raise ValueError("Release builder target must be an owned private directory")
    return info.st_dev, info.st_ino


@contextmanager
def owned(root, *, clean):
    if clean:
        path = Path(tempfile.mkdtemp(prefix="hive-release-"))
    else:
        parent = Path(root) / "target"
        parent.mkdir(exist_ok=True)
        path = parent / ".release-builder"
        path.mkdir(mode=0o700, exist_ok=True)
        sync_directory(parent)
    original = admit(path)
    with _owned_fd(os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)) as owner:
        if (os.fstat(owner).st_dev, os.fstat(owner).st_ino) != original:
            raise ValueError("Release builder directory ownership changed")
        retained = False
        with finalizer(lambda: cleanup(path, original) if clean and not retained else None):
            try:
                with directory_lock(path):
                    if admit(path) != original:
                        raise ValueError("Release builder target changed during acquisition")
                    yield path.resolve()
                    if admit(path) != original:
                        raise ValueError("Release builder target changed during freeze")
            except BaseException as error:
                if getattr(error, "owned_cleanup_uncertain", False):
                    retained = True
                    error.retained_root = str(path)
                raise


def cleanup(path, original):
    if admit(path) != original:
        raise ValueError("Release builder target changed; retaining cleanup evidence")
    shutil.rmtree(path)
