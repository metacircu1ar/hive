"""Deterministic working-tree identity with private same-host change evidence."""

import hashlib
import json
import os
import stat
from pathlib import Path
from .files import _owned_fd
from .cancellation import check


def identity(info):
    return (
        info.st_dev,
        info.st_ino,
        info.st_mode,
        info.st_size,
        info.st_mtime_ns,
        info.st_ctime_ns,
    )


def check_directories(root, path):
    for parent in reversed(path.relative_to(root).parents):
        if parent != Path(".") and (root / parent).is_symlink():
            raise ValueError("Release source directories must not be symlinks")


def names(run, arguments):
    output = run(["git", "ls-files", "-z", *arguments], strip=False, text=False)
    return [os.fsdecode(name) for name in output.split(b"\0") if name]


def ignored_paths(run):
    from . import policy

    prefixes = {*policy.SOURCE_ROOTS, "tests", "scripts/release", "docs", "examples", "skills"}
    prefixes.update(f"{package}/tests" for package in policy.PACKAGES)
    prefixes.update(f"{package}/Cargo.toml" for package in policy.PACKAGES)
    for fixed in (
        policy.ROOT_FILES,
        policy.BINARY_FILES,
        policy.RELEASE_CORPUS,
        policy.ICONS,
        policy.PRODUCT_PYTHON,
        policy.MOCKS,
        policy.GUIDES,
        policy.EXAMPLES,
        ("logo.png", "comb/logo.png", "packaging/install.sh"),
    ):
        prefixes.update(fixed)
    return names(run, ["--others", "--ignored", "--exclude-standard", "--", *sorted(prefixes)])


def item(root, name):
    path = root / name
    check_directories(root, path)
    try:
        before = path.lstat()
    except FileNotFoundError:
        return {"kind": "deleted"}, None
    mode = stat.S_IMODE(before.st_mode)
    if stat.S_ISLNK(before.st_mode):
        target = os.readlink(path)
        resolved = path.resolve(strict=True)
        if not resolved.is_relative_to(root) or not resolved.is_file():
            raise ValueError("Release source symlinks must name regular files inside the checkout")
        content, stamp = regular(resolved)
        record = {"kind": "symlink", "mode": mode, "target": target, **content}
        stamp = identity(before), stamp
    elif stat.S_ISREG(before.st_mode):
        content, stamp = regular(path)
        record = {"kind": "file", "mode": mode, **content}
    else:
        raise ValueError(
            "Release source must contain only regular files and internal file symlinks"
        )
    if identity(path.lstat()) != identity(before):
        raise ValueError("Release source changed while being read")
    return record, stamp


def regular(path):
    with _owned_fd(os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)) as descriptor:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode):
            raise ValueError("Release source must be a regular file")
        value, size = hashlib.sha256(), 0
        with os.fdopen(descriptor, "rb", closefd=False) as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                check()
                value.update(chunk)
                size += len(chunk)
                if size > before.st_size:
                    raise ValueError("Release source grew while being hashed")
        after = os.fstat(descriptor)
    if (
        not stat.S_ISREG(before.st_mode)
        or size != before.st_size
        or identity(before) != identity(after)
        or identity(path.stat()) != identity(after)
    ):
        raise ValueError("Release source changed while being hashed")
    return {"sha256": value.hexdigest(), "bytes": size}, identity(after)


class Snapshot:
    def __init__(self, root, run, *, exclude=()):
        from .policy import allowed_path

        self.root, self.run = Path(root).resolve(), run
        self.exclude = tuple(Path(path).resolve() for path in exclude)
        tracked = names(run, [])
        extra = names(run, ["--others", "--exclude-standard"]) + ignored_paths(run)
        extra = [
            name
            for name in extra
            if name
            and allowed_path(name)
            and not any((self.root / name).absolute().is_relative_to(path) for path in self.exclude)
        ]
        self.records, self.stamps = {}, {}
        for name in sorted(set(tracked + extra) - {""}):
            self.records[name], self.stamps[name] = item(self.root, name)
        encoded = json.dumps(self.records, sort_keys=True, separators=(",", ":")).encode()
        self.public = {
            "commit": run(["git", "rev-parse", "HEAD"]),
            "dirty": bool(extra or run(["git", "status", "--porcelain", "--untracked-files=no"])),
            "tree_sha256": hashlib.sha256(encoded).hexdigest(),
        }

    def check(self):
        current = Snapshot(self.root, self.run, exclude=self.exclude)
        if self.public != current.public or self.stamps != current.stamps:
            raise ValueError("Source changed during build or freeze; attribution refused")

    def check_bytes(self, name, data):
        if name not in self.records or self.records[name]["kind"] == "deleted":
            raise ValueError("Product input is not included in the admitted source snapshot")
        record = self.records[name]
        if record["sha256"] != hashlib.sha256(data).hexdigest() or record["bytes"] != len(data):
            raise ValueError("Frozen product input differs from the admitted source snapshot")

    def check_path(self, path):
        path = Path(path).absolute()
        if not path.is_relative_to(self.root) or not path.resolve().is_relative_to(self.root):
            raise ValueError("Release path dependencies must be inside the source checkout")
        check_directories(self.root, path)
        path = Path(os.path.abspath(path))
        record = self.records.get(path.relative_to(self.root).as_posix())
        if record is None or record["kind"] == "deleted":
            raise ValueError("Release dependency input is not bound to the source snapshot")
