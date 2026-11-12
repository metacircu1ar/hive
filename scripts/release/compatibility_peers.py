"""Fetch only declared release peers, with archive and consumed-member pins."""

import hashlib
import gzip
import io
import json
import os
import re
import stat
import tarfile
from .download import fetch

from .compatibility_gate import decode, fields, path
from .files import atomic_write_bytes, create_directory, _owned_fd
from .common import digest, PARTS
from .compatibility_recipe import recipe

LIMIT = 256 * 1024 * 1024


def catalog(data):
    rows = decode(data)
    if not isinstance(rows, list):
        raise ValueError("Release peers must be a list")
    seen = set()
    for row in rows:
        fields(row, ("version", "target", "parts", "corpus_sha256"))
        if not isinstance(row["corpus_sha256"], str) or not re.fullmatch(
            "[0-9a-f]{64}", row["corpus_sha256"]
        ):
            raise ValueError("Release corpus needs a SHA-256 pin")
        if not isinstance(row["version"], str) or not re.fullmatch(
            r"[0-9]+\.[0-9]+\.[0-9]+", row["version"]
        ):
            raise ValueError("Peer requires a declared product version")
        if row["target"] not in (
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
        ):
            raise ValueError("Unknown peer target")
        key = (row["version"], row["target"])
        if key in seen:
            raise ValueError("Duplicate release peer")
        seen.add(key)
        fields(row["parts"], ("hive", "comb"))
        for part, asset in row["parts"].items():
            validate_asset(row["version"], part, asset)
    return rows


def fetch_local(directory, url, expected):
    """Read a pinned local asset; absence never falls back to the network."""
    name = url.rsplit("/", 1)[-1]
    if name in {".", ".."} or not re.fullmatch(r"[A-Za-z0-9_.-]+", name):
        raise ValueError("Invalid local peer asset name")
    with _owned_fd(
        os.open(directory / name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    ) as descriptor:
        if not stat.S_ISREG(os.fstat(descriptor).st_mode):
            raise ValueError("Local peer asset must be a regular file")
        data = bytearray()
        while len(data) <= LIMIT:
            chunk = os.read(descriptor, min(1024 * 1024, LIMIT + 1 - len(data)))
            if not chunk:
                break
            data.extend(chunk)
        if len(data) > LIMIT:
            raise ValueError("Release peer exceeds download bound")
        if hashlib.sha256(data).hexdigest() != expected:
            raise ValueError("Local peer differs from its declared SHA-256 pin")
        return bytes(data)


def provisional(root, parts, assets, target, source):
    """Derive one new peer from frozen plain builds, without changing source."""
    row = {"version": None, "target": target, "parts": {}, "corpus_sha256": None}
    for part, names in [("hive", ("hivectl", "hive-daemon")), ("comb", ("comb",))]:
        frozen = parts / part
        record = decode((frozen / "build.json").read_bytes())
        package, flags, _, _ = PARTS[part]
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
        if (
            record["part"] != part
            or record["target"] != target
            or record["source"] != source
            or record["profile"] != "release"
            or record["command"] != command
        ):
            raise ValueError("Provisional peer build identity mismatch")
        binary_pins = {}
        for name in names:
            binary = record["binaries"][name]
            profile = binary["cargo_profile"]
            if (
                profile["test"] is not False
                or profile["opt_level"] != "3"
                or profile["debug_assertions"] is not False
                or digest(frozen / "release" / name) != binary["sha256"]
            ):
                raise ValueError("Provisional peer requires verified plain release binaries")
            if row["version"] is None:
                row["version"] = binary["product_version"]
            if binary["product_version"] != row["version"]:
                raise ValueError("Provisional peer product versions disagree")
            binary_pins[name] = {"member": "bin/" + name, "sha256": binary["sha256"]}
        archive = assets / f"{part}-{target}.tar.gz"
        row["parts"][part] = {
            "url": (
                f"https://github.com/metacircu1ar/hive/releases/download/v{row['version']}/"
                f"{archive.name}"
            ),
            "sha256": digest(archive),
            "binaries": binary_pins,
        }
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", row["version"]):
        raise ValueError("Provisional peer requires a release product version")
    row["corpus_sha256"] = digest(root / "compat" / row["version"] / "corpus.json")
    recipe(root, row["version"], expected_sha256=row["corpus_sha256"])
    existing = catalog((root / "compat/releases.json").read_bytes())
    if any((old["version"], old["target"]) == (row["version"], target) for old in existing):
        raise ValueError("An existing release peer cannot be replaced")
    combined = catalog(json.dumps([*existing, row]))
    declarations(root, combined, target)
    return combined


def bootstrap(root, parts, assets, current):
    """Assemble the current default binaries and an offline first-release catalog."""
    from .common import native_target, source_identity
    from .files import atomic_write_json

    rows = provisional(root, parts, assets, native_target(), source_identity())
    create_directory(current)
    for part, names in [("hive", ("hivectl", "hive-daemon")), ("comb", ("comb",))]:
        for name in names:
            atomic_write_bytes(
                current / name,
                (parts / part / "release" / name).read_bytes(),
                mode=0o700,
                create_once=True,
            )
    peers = current.parent / "peers.json"
    atomic_write_json(peers, rows)
    return peers


def declarations(root, rows, target):
    """A peer list cannot silently omit a declared release or invent one."""
    inventory = decode((root / "scripts/release/compatibility.json").read_bytes())
    versions = set()
    for item in inventory["registries"]:
        for family in decode(path(root, item["path"]).read_bytes()):
            versions.update(release["product_version"] for release in family["released"])
    if {row["version"] for row in rows} != versions or {
        row["version"] for row in rows if row["target"] == target
    } != versions:
        raise ValueError("Release peer pins must cover every declared release on this target")
    for version in versions:
        pins = {row["corpus_sha256"] for row in rows if row["version"] == version}
        if len(pins) != 1:
            raise ValueError("Native peers must share one frozen release corpus")
        recipe(root, version, expected_sha256=pins.pop())


def materialize(row, destination, download=fetch):
    """Never extract an archive: consume only uniquely named regular pinned files."""
    create_directory(destination)
    for asset in row["parts"].values():
        data = download(asset["url"], asset["sha256"])
        if len(data) > LIMIT or hashlib.sha256(data).hexdigest() != asset["sha256"]:
            raise ValueError("Release peer archive checksum mismatch")
        with gzip.GzipFile(fileobj=io.BytesIO(data)) as compressed:
            expanded = compressed.read(2 * LIMIT + 1)
        if len(expanded) > 2 * LIMIT:
            raise ValueError("Expanded peer archive exceeds its bound")
        with tarfile.open(fileobj=io.BytesIO(expanded), mode="r:") as archive:
            members = archive.getmembers()
            for name, pin in asset["binaries"].items():
                matches = [member for member in members if member.name == pin["member"]]
                if len(matches) != 1 or not matches[0].isreg() or not 0 < matches[0].size <= LIMIT:
                    raise ValueError("Missing, ambiguous or unsafe peer executable")
                with archive.extractfile(matches[0]) as source:
                    binary = source.read(LIMIT + 1)
                if (
                    len(binary) != matches[0].size
                    or hashlib.sha256(binary).hexdigest() != pin["sha256"]
                ):
                    raise ValueError("Release peer executable checksum mismatch")
                atomic_write_bytes(destination / name, binary, mode=0o700, create_once=True)


def validate_asset(version, part, asset):
    fields(asset, ("url", "sha256", "binaries"))
    prefix = "https://github.com/metacircu1ar/hive/releases/download/v" + version + "/"
    if (
        not isinstance(asset["url"], str)
        or not asset["url"].startswith(prefix)
        or not re.fullmatch(r"[A-Za-z0-9_.-]+", asset["url"][len(prefix) :])
    ):
        raise ValueError("Peer URL must name its immutable GitHub release asset")
    fields(asset["binaries"], ("hivectl", "hive-daemon") if part == "hive" else ("comb",))
    pins = [asset["sha256"]]
    for name, binary in asset["binaries"].items():
        fields(binary, ("member", "sha256"))
        if binary["member"] != "bin/" + name:
            raise ValueError("Unexpected executable member")
        pins.append(binary["sha256"])
    if any(not isinstance(pin, str) or not re.fullmatch("[0-9a-f]{64}", pin) for pin in pins):
        raise ValueError("Every peer asset and executable needs a SHA-256 pin")
