"""Bounded build records and the exact shipping part contracts."""

import hashlib
import json
import os
from pathlib import Path
import re
import stat
from . import llama
from .common import PARTS
from .files import _owned_fd
from .frozen_inputs import validate
from .source import identity

FEATURES = {"hive": ["comb", "default"], "hive-core": [], "comb": [], "hive-install": []}
TARGETS = {
    f"{arch}-{platform}"
    for arch in ("aarch64", "x86_64")
    for platform in ("apple-darwin", "unknown-linux-gnu")
}


def bytes_record(data):
    return {"sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}


def read(path, *, limit=4 * 1024 * 1024):
    path = Path(path)
    with _owned_fd(os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)) as fd:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode) or not 0 <= before.st_size <= limit:
            raise ValueError("Release record must be a bounded regular file")
        with os.fdopen(fd, "rb", closefd=False) as stream:
            data = stream.read(limit + 1)
        if (
            len(data) != before.st_size
            or identity(before) != identity(os.fstat(fd))
            or identity(before) != identity(path.lstat())
        ):
            raise ValueError("Release record changed while being read")
    return data


def unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Duplicate release record key")
        result[key] = value
    return result


def decode(data):
    return json.loads(data, object_pairs_hook=unique)


def metadata(record, *, mode=False):
    keys = {"sha256", "bytes", "mode"} if mode else {"sha256", "bytes"}
    if (
        not isinstance(record, dict)
        or set(record) != keys
        or not isinstance(record["sha256"], str)
        or not re.fullmatch(r"[a-f0-9]{64}", record["sha256"])
        or type(record["bytes"]) is not int
        or not 0 <= record["bytes"] <= 2**31
        or mode
        and (type(record["mode"]) is not int or record["mode"] not in (0o644, 0o755))
    ):
        raise ValueError("Invalid release file identity")


def source(record):
    if (
        not isinstance(record, dict)
        or set(record) != {"commit", "dirty", "tree_sha256"}
        or type(record["dirty"]) is not bool
        or not isinstance(record["commit"], str)
        or not re.fullmatch(r"[a-f0-9]{40}", record["commit"])
        or not isinstance(record["tree_sha256"], str)
        or not re.fullmatch(r"[a-f0-9]{64}", record["tree_sha256"])
    ):
        raise ValueError("Invalid release source identity")


def product_version(value):
    if not isinstance(value, str) or not re.fullmatch(r"[0-9][0-9A-Za-z.+-]{0,127}", value):
        raise ValueError("Invalid release product version")


def contract(build):
    part, target = build["part"], build["target"]
    if part not in PARTS or target not in TARGETS:
        raise ValueError("Unknown shipping part or target")
    if part == llama.PART:
        return llama_contract(build)
    package, flags, names, _ = PARTS[part]
    command = [
        "cargo",
        "build",
        "--release",
        "--locked",
        "-p",
        package,
        *flags,
        "--message-format=json-render-diagnostics",
    ]
    if build["profile"] != "release" or build["command"] != command:
        raise ValueError("Require a native plain package-scoped release build record")
    if set(build["binaries"]) != set(names):
        raise ValueError("Build record has the wrong binary set")
    source(build["source"])
    validate(build["inputs"])
    versions = {info["product_version"] for info in build["binaries"].values()}
    if len(versions) != 1:
        raise ValueError("Build record has inconsistent product versions")
    version = next(iter(versions))
    product_version(version)
    for name in names:
        binary_contract(build["binaries"][name], part, name, version)
    return version


def llama_contract(build):
    """The non-Cargo part: the pinned CMake build of one llama-server binary."""
    if build["profile"] != "release" or build["command"] != llama.COMMAND:
        raise ValueError("Require a native plain release llama-server build record")
    if set(build["binaries"]) != {llama.BINARY}:
        raise ValueError("Build record has the wrong binary set")
    source(build["source"])
    validate(build["inputs"])
    llama.check_record(build["llama"], build["target"])
    info = build["binaries"][llama.BINARY]
    if not isinstance(info, dict) or set(info) != {
        "bytes",
        "sha256",
        "version",
        "product_version",
        "dependencies",
    }:
        raise ValueError("Invalid llama-server binary record")
    product_version(info["product_version"])
    llama.check_version(info["version"], build["llama"]["commit"])
    metadata({key: info[key] for key in ("sha256", "bytes")})
    return info["product_version"]


def binary_contract(info, part, name, version):
    profile = info["cargo_profile"]
    if (
        profile.get("test") is not False
        or profile.get("opt_level") != "3"
        or profile.get("debug_assertions") is not False
        or info["cargo_features"] != FEATURES[part]
    ):
        raise ValueError("Binary lacks the intended plain release profile or features")
    variant = PARTS[part][3]
    expected = f"{name} {version}" + (f" ({variant})" if variant else "")
    if info["version"] != expected:
        raise ValueError("Binary version or variant does not match its shipping part")
    metadata({key: info[key] for key in ("sha256", "bytes")})


def member_names(part, target):
    name = f"{part}-{target}"
    return (
        {name: 0o755, name + ".LICENSES": 0o644, name + ".build.json": 0o644}
        if part == "hive-install"
        else {name + ".tar.gz": 0o644}
    )
