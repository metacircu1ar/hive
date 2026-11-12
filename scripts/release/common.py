"""Shared, unversioned release layout and native dependency inspection."""

import hashlib
import os
import platform
import re
import struct
from pathlib import Path
from .files import atomic_write_json
from .native import macos_minimum
from .source import Snapshot
from .process import run as run_process

ROOT = Path(__file__).resolve().parents[2]
# Part: (Cargo package, flags, binaries, variant). comb-llama is the one non-Cargo
# part: a CMake build of the pinned llama-server, so it has no package or variant.
PARTS = {
    "hive": ("hive", [], ["hivectl", "hive-daemon"], "comb"),
    "hive-core": ("hive", ["--no-default-features"], ["hivectl", "hive-daemon"], "core"),
    "comb": ("comb", [], ["comb"], "http"),
    "comb-llama": (None, [], ["comb-llama-server"], None),
    "hive-install": ("hive-install", [], ["hive-install"], None),
}


def component(part):
    """The installed component, which owns the share/COMPONENT payload directory."""
    if part.startswith("hive"):
        return "hive"
    return "comb-llama" if part == "comb-llama" else "comb"


def run(args, *, strip=True, timeout=60, text=True, **kwargs):
    output = run_process(args, cwd=ROOT, text=text, timeout=timeout, check=True, **kwargs).stdout
    return output.strip() if strip else output


def digest(path):
    with Path(path).open("rb") as stream:
        value = hashlib.sha256()
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def native_target():
    arch = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64"}.get(platform.machine())
    suffix = {"Darwin": "apple-darwin", "Linux": "unknown-linux-gnu"}.get(platform.system())
    if not arch or not suffix:
        raise ValueError("Only native macOS and GNU/Linux arm64/x86_64 releases are supported")
    return f"{arch}-{suffix}"


LINUX_SYSTEM_LIBRARY_ROOTS = (Path("/lib"), Path("/lib64"), Path("/usr/lib"))


def dependencies_match(recorded, observed, target):
    """Compare the runtime closure while permitting Linux system-path aliases."""
    if not target.endswith("-linux-gnu"):
        return recorded == observed
    if not isinstance(recorded, dict) or not isinstance(observed, dict):
        return False
    if set(recorded) != {"libraries", "required_glibc"} or set(observed) != set(recorded):
        return False
    expected, actual = recorded["libraries"], observed["libraries"]
    if (
        recorded["required_glibc"] != observed["required_glibc"]
        or not isinstance(expected, list)
        or not isinstance(actual, list)
        or len(expected) != len(actual)
    ):
        return False
    for left, right in zip(expected, actual, strict=True):
        if not isinstance(left, str) or not isinstance(right, str):
            return False
        try:
            paths = [Path(left), Path(right)]
            if any(not path.is_absolute() for path in paths):
                return False
            resolved = [path.resolve(strict=True) for path in paths]
            if any(
                not path.is_file()
                or not any(path.is_relative_to(root) for root in LINUX_SYSTEM_LIBRARY_ROOTS)
                for path in resolved
            ) or not os.path.samefile(paths[0], paths[1]):
                return False
        except (OSError, RuntimeError):
            return False
    return True


def inspect_binary(path, target, *, env=None):
    """Reject non-native files and non-system dynamic dependency closures."""
    arm = target.startswith("aarch64-")
    with Path(path).open("rb") as stream:
        header = stream.read(32)
        if len(header) != 32:
            raise ValueError("Truncated native executable header")
        if "apple" in target:
            if header[:4] != b"\xcf\xfa\xed\xfe" or struct.unpack_from("<I", header, 4)[0] != (
                0x100000C if arm else 0x1000007
            ):
                raise ValueError(f"Wrong Mach-O architecture: {path}")
            minimum = macos_minimum(stream, header, binary_name=Path(path).name)
    if "apple" in target:
        libraries = [
            line.strip().split(" (", 1)[0]
            for line in run(["otool", "-L", str(path)], env=env).splitlines()[1:]
        ]
        if any(not name.startswith(("/usr/lib/", "/System/Library/")) for name in libraries):
            raise ValueError(f"Non-system shared library in {path}: {libraries}")
        return {"libraries": libraries, "required_glibc": None, "required_macos": minimum}
    if header[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<H", header, 18)[0] != (
        183 if arm else 62
    ):
        raise ValueError(f"Wrong ELF architecture: {path}")
    listing = run(["ldd", str(path)], env=env)
    libraries = []
    for line in listing.splitlines():
        if "linux-vdso" in line:
            continue
        name = line.split("=>", 1)[-1].strip().split(" (", 1)[0]
        if not name.startswith(("/lib/", "/lib64/", "/usr/lib/")):
            raise ValueError(f"Missing or non-system dependency: {line}")
        libraries.append(name)
    symbols = run(["readelf", "--version-info", str(path)], env=env)
    versions = re.findall(r"\bGLIBC_([0-9]+\.[0-9]+(?:\.[0-9]+)?)\b", symbols)
    if not versions:
        raise ValueError("GNU/Linux release lacks a verifiable GLIBC requirement")
    # Release programs are Rust with C dependencies only; a C++ runtime would be a
    # requirement the installer does not check.
    if re.search(r"\b(GLIBCXX|CXXABI)_[0-9]", symbols) or any(
        Path(name).name.startswith(("libstdc++", "libc++")) for name in libraries
    ):
        raise ValueError(f"Release binaries must not require the C++ runtime: {path}")
    return {
        "libraries": libraries,
        "required_glibc": max(versions, key=lambda v: tuple(map(int, v.split(".")))),
    }


def source_identity():
    return Snapshot(ROOT, run).public


def write_json(path, value):
    atomic_write_json(path, value, mode=0o644)
