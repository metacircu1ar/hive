"""One authoritative, source-bound native asset ledger under the publication lock."""

from pathlib import Path
import stat
from . import files
from .archive import json_bytes
from .build_record import (
    PARTS,
    TARGETS,
    bytes_record,
    decode,
    member_names,
    metadata,
    product_version,
    read,
    source,
)
from .source import regular

LEDGER = "assets.json"


def file_record(path):
    path = Path(path)
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or not 0 <= info.st_size <= 2**31:
        raise ValueError("Release assets must be bounded regular files")
    record, _ = regular(path)
    result = record | {"mode": path.stat().st_mode & 0o777}
    metadata(result, mode=True)
    return result


def members(ledger):
    result = {"install.sh": ledger["bootstrap"]}
    for contribution in ledger["builds"].values():
        result.update(contribution["files"])
    return result


def sums(raw, identities):
    identities = identities | {LEDGER: bytes_record(raw)}
    return "".join(f"{record['sha256']}  {name}\n" for name, record in sorted(identities.items()))


def validate(ledger):
    if (
        not isinstance(ledger, dict)
        or set(ledger)
        != {
            "format",
            "source",
            "product_version",
            "inputs_sha256",
            "bootstrap",
            "builds",
            "previous_checksums_sha256",
        }
        or type(ledger["format"]) is not int
        or ledger["format"] != 1
    ):
        raise ValueError("Unsupported asset ledger")
    source(ledger["source"])
    product_version(ledger["product_version"])
    metadata(ledger["bootstrap"], mode=True)
    metadata({"sha256": ledger["inputs_sha256"], "bytes": 0})
    if ledger["bootstrap"]["mode"] != 0o755:
        raise ValueError("Invalid bootstrap mode")
    previous = ledger["previous_checksums_sha256"]
    if previous is not None:
        metadata({"sha256": previous, "bytes": 0})
    if not isinstance(ledger["builds"], dict) or not 1 <= len(ledger["builds"]) <= 16:
        raise ValueError("Invalid asset contribution count")
    for key, contribution in ledger["builds"].items():
        part, separator, target = key.partition("/")
        if not separator or part not in PARTS or target not in TARGETS:
            raise ValueError("Invalid asset contribution")
        if not isinstance(contribution, dict) or set(contribution) != {"build", "files"}:
            raise ValueError("Invalid asset contribution fields")
        metadata(contribution["build"])
        expected = member_names(part, target)
        if set(contribution["files"]) != set(expected):
            raise ValueError("Invalid asset contribution members")
        for name, record in contribution["files"].items():
            metadata(record, mode=True)
            if record["mode"] != expected[name]:
                raise ValueError("Invalid asset member mode")


def load(output, *, repair=False):
    output = Path(output)
    present = {path.name for path in output.iterdir()} - {".publication.lock"}
    if LEDGER not in present:
        if present:
            raise ValueError("Existing assets lack a coherent ledger; use a fresh output directory")
        return None
    raw = read(output / LEDGER, limit=16 * 1024 * 1024)
    ledger = decode(raw)
    validate(ledger)
    identities = members(ledger)
    if present - {LEDGER, "SHA256SUMS"} != set(identities):
        raise ValueError("Asset directory has missing or unknown members")
    for name, record in identities.items():
        if file_record(output / name) != record:
            raise ValueError("Asset member differs from its committed ledger")
    expected = sums(raw, identities).encode()
    current = read(output / "SHA256SUMS", limit=65536) if "SHA256SUMS" in present else None
    if current != expected and not (
        repair
        and (
            current is None
            or bytes_record(current)["sha256"] == ledger["previous_checksums_sha256"]
        )
    ):
        raise ValueError("Asset checksum index does not match the committed ledger")
    return ledger


def contribute(output, build, raw, publish):
    ledger = load(output, repair=True)
    version = next(iter(build["binaries"].values()))["product_version"]
    bootstrap = build["inputs"]["packaging/install.sh"]
    inputs = bytes_record(json_bytes(build["inputs"]))["sha256"]
    key = build["part"] + "/" + build["target"]
    if ledger is not None:
        if (
            ledger["source"] != build["source"]
            or ledger["product_version"] != version
            or ledger["inputs_sha256"] != inputs
            or ledger["bootstrap"] != bootstrap
        ):
            raise ValueError("Cannot mix source, version or product inputs in an asset set")
        if key in ledger["builds"]:
            if ledger["builds"][key]["build"] != bytes_record(raw):
                raise ValueError("An asset contribution cannot be replaced by a different build")
            commit_checksums(output, ledger)
            return
    else:
        ledger = {
            "format": 1,
            "source": build["source"],
            "product_version": version,
            "inputs_sha256": inputs,
            "bootstrap": bootstrap,
            "builds": {},
            "previous_checksums_sha256": None,
        }
    previous = output / "SHA256SUMS"
    ledger["previous_checksums_sha256"] = (
        bytes_record(read(previous, limit=65536))["sha256"] if previous.exists() else None
    )
    expected = publish()
    if set(expected) != set(member_names(build["part"], build["target"])):
        raise ValueError("Published asset contribution has the wrong members")
    for name, record in expected.items():
        if file_record(output / name) != record:
            raise ValueError("Published asset differs from its consumed snapshot")
    contribution = {"build": bytes_record(raw), "files": expected}
    if file_record(output / "install.sh") != bootstrap:
        raise ValueError("Published bootstrap differs from its frozen input")
    ledger["builds"][key] = contribution
    validate(ledger)
    files.atomic_write_bytes(output / LEDGER, json_bytes(ledger), mode=0o644)
    commit_checksums(output, ledger)


def commit_checksums(output, ledger):
    raw = read(output / LEDGER, limit=16 * 1024 * 1024)
    if decode(raw) != ledger:
        raise ValueError("Asset ledger changed before checksum publication")
    files.atomic_write_text(output / "SHA256SUMS", sums(raw, members(ledger)), mode=0o644)
