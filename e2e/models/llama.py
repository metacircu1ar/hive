"""The pinned static llama-server: the installed one, or the same build in a private cache.

An installed `comb-llama-server` whose version names the pinned commit is used as
is. A source checkout without it builds the release part's exact CMake options from
the same commit-and-tree pin into a private, regenerable user cache.
"""

import json
from pathlib import Path
import shutil

from e2e.cache import cache_directory
from e2e.scripts.harness.hashing import sha256
from release import llama as release
from release.build_record import read
from release.cancellation import check, pause
from release.common import inspect_binary, native_target
from release.files import atomic_copy, atomic_write_json, create_directory, record_lock
from release.process import ProcessFailure
from release.progress import phase
from release.temporary import directory as temporary_directory

NAME = release.BINARY


def installed(search=None):
    """An installed comb-llama-server on PATH that reports the pinned commit, if any.

    A server that reports another commit, prints no version line or fails
    `--version` is not the pinned build, so callers use the cached build instead.
    """
    found = shutil.which(NAME, path=search)
    if found is None:
        return None
    commit = release.load()["llama_cpp"]["commit"]
    path = Path(found).resolve()
    try:
        release.check_version(release.version(path), commit)
    except (ProcessFailure, ValueError):
        return None
    return path


def cached(directory):
    return Path(directory) / "llama" / NAME


def receipt(path):
    """Everything that selects a cached build: pin, target, options and binary bytes."""
    target = native_target()
    return dict(
        source=release.load()["llama_cpp"],
        target=target,
        options=release.options(target),
        sha256=sha256(path),
    )


def ready(directory):
    path = cached(directory)
    record = path.with_suffix(".json")
    if not path.is_file() or not record.is_file():
        return False
    value = json.loads(read(record, limit=64 * 1024))
    return all(value.get(key) == expected for key, expected in receipt(path).items())


def build(directory):
    """Build the release part's exact binary into the cache, once per pin and target."""
    directory = cache_directory(directory)
    with record_lock(directory / "llama-build", check=check, pause=pause):
        if ready(directory):
            return cached(directory)
        pin = release.load()
        target = native_target()
        env = release.cache_environment()
        with temporary_directory(prefix="llama-build-", parent=directory) as work:
            with phase("build"):
                executable, identity = release.build(work, pin, target, env)
            release.check_version(
                release.version(executable, env=env), pin["llama_cpp"]["commit"]
            )
            candidate = Path(work) / NAME
            atomic_copy(executable, candidate, mode=0o700)
            dependencies = inspect_binary(candidate, target, env=env)
            path = cached(directory)
            create_directory(path.parent, exist_ok=True)
            atomic_copy(candidate, path, mode=0o700)
            atomic_write_json(
                path.with_suffix(".json"),
                dict(receipt(path), identity=identity, dependencies=dependencies),
            )
        return path
