"""Freeze all non-executable release payloads before declaring a build complete."""

import hashlib
import os
import re
import stat
from pathlib import Path, PurePosixPath
from . import files
from .docs import product_files, GUIDES, validate_links


def record(data, mode):
    return {"sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data), "mode": mode}


def freeze(root, output, source):
    payloads = product_files(root)
    payloads["packaging/install.sh"] = (root / "packaging/install.sh").read_bytes()
    records = {}
    for name, data in sorted(payloads.items()):
        source.check_bytes(name, data)
        mode = 0o755 if name == "packaging/install.sh" else 0o644
        path = output / "inputs" / name
        files.create_directory(path.parent, exist_ok=True)
        files.atomic_write_bytes(path, data, mode=mode, create_once=True)
        records[name] = record(data, mode)
    validate(records)
    return records


def validate(records):
    required = {
        *GUIDES,
        "logo.png",
        "comb/logo.png",
        "examples/qwen3-embedding.json",
        "packaging/install.sh",
    }
    if not isinstance(records, dict) or len(records) > 4096 or not required.issubset(records):
        raise ValueError("Frozen build lacks complete product inputs")
    total = 0
    for name, metadata in records.items():
        if not isinstance(name, str):
            raise ValueError("Invalid frozen product-input path")
        path = PurePosixPath(name)
        example = name.startswith("examples/") and path.suffix in {".md", ".yaml"}
        if (
            path.is_absolute()
            or ".." in path.parts
            or str(path) != name
            or name not in required
            and not example
        ):
            raise ValueError("Invalid frozen product-input path")
        if (
            not isinstance(metadata, dict)
            or set(metadata) != {"sha256", "bytes", "mode"}
            or type(metadata["bytes"]) is not int
            or metadata["bytes"] < 0
            or metadata["bytes"] > 512 * 1024 * 1024
            or not isinstance(metadata["sha256"], str)
            or not re.fullmatch(r"[0-9a-f]{64}", metadata["sha256"])
            or type(metadata["mode"]) is not int
            or metadata["mode"] != (0o755 if name == "packaging/install.sh" else 0o644)
        ):
            raise ValueError("Invalid frozen product-input record")
        total += metadata["bytes"]
    if total > 512 * 1024 * 1024:
        raise ValueError("Frozen product inputs exceed the native payload limit")


def product_payloads(directory, records):
    validate(records)
    payloads, contents = {}, {}
    for name, metadata in records.items():
        if name == "packaging/install.sh":
            continue
        path = Path(directory) / "inputs" / name
        with files._owned_fd(os.open(path, os.O_RDONLY | os.O_NONBLOCK)) as descriptor:
            info = os.fstat(descriptor)
            if not stat.S_ISREG(info.st_mode) or info.st_size != metadata["bytes"]:
                raise ValueError("Frozen product input must be a regular file of its declared size")
            with os.fdopen(descriptor, "rb", closefd=False) as stream:
                data = stream.read(metadata["bytes"] + 1)
        if record(data, metadata["mode"]) != metadata:
            raise ValueError("Frozen product input identity mismatch")
        contents[name], payloads[name] = data, (path, metadata)
    validate_links(contents)
    return payloads
