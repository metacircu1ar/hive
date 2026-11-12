"""Admit a small, output-neutral Cargo configuration grammar on Python 3.10+."""

import hashlib
import json
import os
import re
import stat
from .files import _owned_fd

LIMIT = 1024 * 1024
KEYS = {
    "build": {"jobs": "positive", "incremental": bool},
    "net": {"git-fetch-with-cli": bool, "offline": bool, "retry": "nonnegative"},
    "http": {
        "proxy": str,
        "cainfo": str,
        "timeout": "positive",
        "low-speed-limit": "positive",
        "multiplexing": bool,
        "check-revoke": bool,
    },
    "term": {"color": ("auto", "always", "never"), "verbose": bool, "quiet": bool},
    "registries.crates-io": {"protocol": ("sparse", "git")},
}


def without_comment(line):
    quote, escaped = None, False
    for index, character in enumerate(line):
        if escaped:
            escaped = False
        elif quote == '"' and character == "\\":
            escaped = True
        elif character == quote:
            quote = None
        elif quote is None and character in "\"'":
            quote = character
        elif quote is None and character == "#":
            return line[:index].strip()
    return line.strip()


def scalar(value):
    if value in ("true", "false"):
        return value == "true"
    if re.fullmatch(r"0|[1-9][0-9]{0,18}", value):
        number = int(value)
        if number <= (1 << 63) - 1:
            return number
        raise ValueError("Cargo configuration integer exceeds the supported range")
    if re.fullmatch(r"'[^'\x00-\x1f]*'", value):
        return value[1:-1]
    if re.fullmatch(r'"(?:[^"\\\x00-\x1f]|\\["\\bfnrt]|\\u[0-9a-fA-F]{4})*"', value):
        result = json.loads(value)
        if not any(0xD800 <= ord(character) <= 0xDFFF for character in result):
            return result
    raise ValueError("Unsupported Cargo configuration value syntax")


def validate(data):
    table, seen_tables, seen_keys = None, set(), set()
    for raw in data.decode("utf-8").splitlines():
        line = without_comment(raw)
        if not line:
            continue
        if line.startswith("["):
            match = re.fullmatch(r"\[([a-z][a-z0-9_.-]*)\]", line)
            if match is None or match[1] not in KEYS or match[1] in seen_tables:
                raise ValueError("Unsupported or duplicate Cargo configuration table")
            table = match[1]
            seen_tables.add(table)
            continue
        match = re.fullmatch(r"([a-z][a-z0-9_-]*)\s*=\s*(.+)", line)
        if match is None or table is None or match[1] not in KEYS[table]:
            raise ValueError("Unsupported Cargo configuration key or syntax")
        key = (table, match[1])
        if key in seen_keys:
            raise ValueError("Duplicate Cargo configuration key")
        seen_keys.add(key)
        value, expected = scalar(match[2]), KEYS[table][match[1]]
        valid = (
            type(value) is int and value > 0
            if expected == "positive"
            else type(value) is int and value >= 0
            if expected == "nonnegative"
            else value in expected
            if isinstance(expected, tuple)
            else type(value) is expected
        )
        if not valid:
            raise ValueError("Unsupported Cargo configuration value")


def metadata(info):
    return (
        info.st_dev,
        info.st_ino,
        info.st_mode,
        info.st_size,
        info.st_mtime_ns,
        info.st_ctime_ns,
    )


def read(path):
    try:
        link = path.lstat()
    except FileNotFoundError:
        return None
    with _owned_fd(os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_CLOEXEC)) as descriptor:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or before.st_size > LIMIT:
            raise ValueError("Cargo configuration must be a bounded regular file")
        with os.fdopen(descriptor, "rb", closefd=False) as stream:
            data = stream.read(LIMIT + 1)
        after = os.fstat(descriptor)
        if (
            len(data) > LIMIT
            or metadata(before) != metadata(after)
            or metadata(path.stat()) != metadata(after)
            or metadata(path.lstat()) != metadata(link)
        ):
            raise ValueError("Cargo configuration changed while being read")
    validate(data)
    return metadata(link), metadata(after), hashlib.sha256(data).digest()


class Snapshot:
    def __init__(self, root, cargo_home):
        directories = [*(parent / ".cargo" for parent in (root, *root.parents)), cargo_home]
        self.files = dict.fromkeys(
            directory / name for directory in directories for name in ("config", "config.toml")
        )
        self.files = {path: read(path) for path in self.files}

    def check(self):
        if any(read(path) != original for path, original in self.files.items()):
            raise ValueError("Cargo configuration changed during the build freeze")
