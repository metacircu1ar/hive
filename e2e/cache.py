"""Private, streamed and checksum-pinned dependency publication."""

import json
import os
from pathlib import Path
import shutil
import stat

from release.cancellation import check, pause
from release.files import create_directory, publish_prepared, raw_log, record_lock
from release.process import run
from release.progress import phase
from e2e.paths import ROOT
from e2e.scripts.harness.hashing import sha256

# Ubuntu images are E2E-only; the model and llama.cpp pins are the product's one copy.
PINS = json.loads((ROOT / "e2e/dependencies.json").read_text())
DOWNLOAD_SECONDS = 3600
DOWNLOAD_HEADROOM = 512 * 1024**2


def model_pin():
    """The shared Qwen pin; a placeholder anywhere in the shared pin refuses it."""
    from release.llama import load

    return load()["model"]


def cache_path(path=None):
    return (
        Path(path or Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "hive-e2e")
        .expanduser()
        .absolute()
    )


def cache_directory(path=None):
    path = cache_path(path)
    create_directory(path, exist_ok=True, mode=0o700)
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise ValueError("The dependency cache must be a private directory owned by this user")
    return path


def verify(path, pin):
    info = path.lstat()
    if (
        not stat.S_ISREG(info.st_mode)
        or info.st_nlink != 1
        or info.st_uid != os.getuid()
        or info.st_mode & 0o022
        or info.st_size != pin["bytes"]
    ):
        raise ValueError("Dependency file ownership, type or byte length is invalid")
    with phase("verify"):
        if sha256(path) != pin["sha256"]:
            raise ValueError("Dependency SHA-256 differs from its pin")
    after = path.lstat()
    if (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns) != (
        after.st_dev,
        after.st_ino,
        after.st_size,
        after.st_mtime_ns,
        after.st_ctime_ns,
    ):
        raise ValueError("Dependency changed during verification")


def fetch(directory, name, url, pin):
    directory = cache_directory(directory)
    if Path(name).name != name:
        raise ValueError("Dependency name must be a single filename")
    model = directory / name
    with record_lock(directory / (name + "-publication"), check=check, pause=pause) as lock:
        if model.exists():
            verify(model, pin)
            return model
        partial = model.with_name(model.name + ".part")
        with raw_log(partial) as output:
            original = os.fstat(output.fileno())
            offset = original.st_size
            if offset > pin["bytes"]:
                raise ValueError("Partial dependency exceeds its declared byte length")
            remaining = pin["bytes"] - offset
            if shutil.disk_usage(directory).free < remaining + DOWNLOAD_HEADROOM:
                raise ValueError(
                    "Insufficient free space for the pinned dependency and download headroom"
                )
            if remaining:
                with phase("fetch"):
                    run(
                        [
                            "curl",
                            "--fail",
                            "--location",
                            "--proto",
                            "=https",
                            "--proto-redir",
                            "=https",
                            "--continue-at",
                            str(offset),
                            "--output",
                            "-",
                            "--max-time",
                            str(DOWNLOAD_SECONDS),
                            "--speed-limit",
                            "1024",
                            "--speed-time",
                            "60",
                            url,
                        ],
                        check=True,
                        stdout=output,
                        pass_fds=(lock,),
                        timeout=DOWNLOAD_SECONDS + 5,
                        limit=remaining + 65536,
                    )
            final = partial.lstat()
            if (original.st_dev, original.st_ino) != (final.st_dev, final.st_ino):
                raise ValueError("Dependency partial name changed during its download")
            verify(partial, pin)
            publish_prepared(partial, model)
        return model
