#!/usr/bin/env python3
"""Assemble a native part from a verified build_release.py freeze; never build here."""

import argparse
import json
from pathlib import Path
from release import llama
from release.common import PARTS, component, digest, inspect_binary, native_target
from release.common import run
from release.frozen_inputs import product_payloads
from release.build_record import bytes_record, contract, decode, read
from release.asset_set import contribute
from release import files as publication
from release.archive import Payload, json_bytes, write_archive
from release.progress import entry
from release.locking import directory_lock
from release.native_evidence import evidence


def package(build_dir, output):
    build_bytes = read(build_dir / "build.json")
    build = decode(build_bytes)
    part = build["part"]
    target = build["target"]
    contract(build)
    if target != native_target():
        raise ValueError("Require a native plain package-scoped release build record")
    names = PARTS[part][2]
    files, glibc = {}, []
    for name in names:
        path = build_dir / "release" / name
        record = build["binaries"][name]
        dependencies = inspect_binary(path, target)
        if (
            digest(path) != record["sha256"]
            or path.stat().st_size != record["bytes"]
            or dependencies != record["dependencies"]
            or reported_version(part, path) != record["version"]
        ):
            raise ValueError(f"Frozen build identity mismatch: {name}")
        if dependencies["required_glibc"]:
            glibc.append(dependencies["required_glibc"])
        files["bin/" + name] = Payload(path, 0o755, record)
    required = max(glibc, key=lambda v: tuple(map(int, v.split(".")))) if glibc else None
    if required and tuple(map(int, required.split("."))) > (2, 35):
        raise ValueError(
            f"Build requires GLIBC {required}; release floor is 2.35. Build on Ubuntu 22.04."
        )
    notice = build_dir / "THIRD_PARTY_LICENSES"
    notice_record = {"sha256": build["notices_sha256"], "bytes": build["notices_bytes"]}
    if digest(notice) != notice_record["sha256"] or notice.stat().st_size != notice_record["bytes"]:
        raise ValueError("Build notice identity mismatch")
    publication.create_directory(output, exist_ok=True)
    with directory_lock(output):

        def publish_members():
            expected = publish(build_dir, output, build_bytes, files, required)
            if not (output / "install.sh").exists():
                publication.atomic_copy(
                    build_dir / "inputs/packaging/install.sh",
                    output / "install.sh",
                    expected=build["inputs"]["packaging/install.sh"],
                    mode=0o755,
                )
            return expected

        contribute(output, build, build_bytes, publish_members)
    print(output / (f"{part}-{target}" + ("" if part == "hive-install" else ".tar.gz")))


def publish(build_dir, output, build_bytes, files, required):
    build = json.loads(build_bytes)
    notice_record = {"sha256": build["notices_sha256"], "bytes": build["notices_bytes"]}
    part, target = build["part"], build["target"]
    if part == "hive-install":
        name = f"hive-install-{target}"
        publication.atomic_copy(
            build_dir / "release/hive-install",
            output / name,
            expected=build["binaries"]["hive-install"],
            mode=0o755,
        )
        publication.atomic_copy(
            build_dir / "THIRD_PARTY_LICENSES",
            output / (name + ".LICENSES"),
            expected=notice_record,
            mode=0o644,
        )
        publication.atomic_write_bytes(output / (name + ".build.json"), build_bytes, mode=0o644)
        return {
            name: {key: build["binaries"]["hive-install"][key] for key in ("sha256", "bytes")}
            | {"mode": 0o755},
            name + ".LICENSES": notice_record | {"mode": 0o644},
            name + ".build.json": bytes_record(build_bytes) | {"mode": 0o644},
        }
    share = f"share/{component(part)}/"
    files[share + "THIRD_PARTY_LICENSES"] = Payload(
        build_dir / "THIRD_PARTY_LICENSES", 0o644, notice_record
    )
    files[share + "build.json"] = Payload(build_bytes, 0o644)
    # The server part carries only its notices and record; Comb ships the guides.
    if part != llama.PART:
        for path, (source, metadata) in product_payloads(build_dir, build["inputs"]).items():
            files[share + path] = Payload(source, metadata["mode"], metadata)
    manifest = {
        "format": 1,
        "product_version": next(iter(build["binaries"].values()))["product_version"],
        "part": part,
        "target": target,
        "required_glibc": required,
        "files": {name: payload.manifest() for name, payload in files.items()},
    }
    files["manifest.json"] = Payload(json_bytes(manifest), 0o644)
    name = f"{part}-{target}.tar.gz"
    record = publication.atomic_stream(
        output / name, lambda stream: write_archive(stream, files), mode=0o644
    )
    return {name: record | {"mode": 0o644}}


def reported_version(part, path):
    """llama-server reports its pinned commit on stderr; Cargo parts print one line."""
    if part == llama.PART:
        return llama.version(path.resolve())
    return run([str(path.resolve()), "--version"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    with evidence(args.output_dir.with_name(args.output_dir.name + ".diagnostics")):
        package(args.build_dir, args.output_dir)


if __name__ == "__main__":
    raise SystemExit(entry(main, name="package"))
