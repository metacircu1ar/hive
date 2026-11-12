"""Snapshot a complete, coherent native set before any bootstrap/helper execution."""

from pathlib import Path
from . import asset_set, files
from .archive import json_bytes
from .build_record import bytes_record, contract, decode, read
from .common import PARTS, dependencies_match, inspect_binary, native_target
from .locking import directory_lock


def admit(assets, destination):
    assets, destination = Path(assets), Path(destination)
    target = native_target()
    with directory_lock(assets):
        ledger = asset_set.load(assets)
        if ledger is None or not {part + "/" + target for part in PARTS}.issubset(ledger["builds"]):
            raise ValueError("Acceptance requires all five native release parts")
        files.create_directory(destination)
        for name, expected in asset_set.members(ledger).items():
            files.atomic_copy(
                assets / name,
                destination / name,
                expected=expected,
                mode=expected["mode"],
                create_once=True,
            )
        for name in (asset_set.LEDGER, "SHA256SUMS"):
            data = read(assets / name, limit=16 * 1024 * 1024)
            files.atomic_write_bytes(destination / name, data, mode=0o644, create_once=True)
        if asset_set.load(destination) != ledger:
            raise ValueError("Asset set changed while being snapshotted")
    helper = verify_helper(destination, ledger, target)
    return destination, helper


def verify_helper(assets, ledger, target):
    name = "hive-install-" + target
    contribution = ledger["builds"]["hive-install/" + target]
    raw = read(assets / (name + ".build.json"))
    build = decode(raw)
    if (
        bytes_record(raw) != contribution["build"]
        or build["part"] != "hive-install"
        or build["target"] != target
    ):
        raise ValueError("Installer helper build record differs from its asset contribution")
    version = contract(build)
    record = build["binaries"]["hive-install"]
    if (
        build["source"] != ledger["source"]
        or version != ledger["product_version"]
        or bytes_record(json_bytes(build["inputs"]))["sha256"] != ledger["inputs_sha256"]
        or build["inputs"]["packaging/install.sh"] != ledger["bootstrap"]
        or any(record[key] != contribution["files"][name][key] for key in ("sha256", "bytes"))
        or build["notices_sha256"] != contribution["files"][name + ".LICENSES"]["sha256"]
        or build["notices_bytes"] != contribution["files"][name + ".LICENSES"]["bytes"]
        or not dependencies_match(
            record["dependencies"], inspect_binary(assets / name, target), target
        )
    ):
        raise ValueError("Installer helper provenance differs from its coherent release set")
    return {
        "sha256": record["sha256"],
        "bytes": record["bytes"],
        "build_sha256": contribution["build"]["sha256"],
        "source": build["source"],
    }
