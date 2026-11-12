"""Fallback parts must come from an accepted native host asset set."""

import hashlib
from pathlib import Path, PurePosixPath
import re
import tarfile

from release import asset_set
from release.common import component
from release.archive import json_bytes
from release.build_record import bytes_record, contract, decode, read
from release.cancellation import check

PARTS = ("hive", "hive-core", "comb", "comb-llama", "hive-install")


def archive_build(path, member):
    found, total, count = None, 0, 0
    with tarfile.open(path, "r|gz") as archive:
        for row in archive:
            check()
            count += 1
            total += row.size
            name = PurePosixPath(row.name)
            if (
                count > 10000
                or not row.isfile()
                or row.size < 0
                or total > 4 * 1024**3
                or name.is_absolute()
                or ".." in name.parts
            ):
                raise ValueError("Fallback archive exceeds its regular-file bounds")
            if row.name == member and (found is not None or not 0 < row.size <= 4 * 1024**2):
                raise ValueError("Fallback archive lacks one bounded build record")
            content = bytearray()
            with archive.extractfile(row) as stream:
                remaining = row.size
                while remaining:
                    check()
                    chunk = stream.read(min(remaining, 1024 * 1024))
                    if not chunk:
                        raise ValueError("Fallback archive member is incomplete")
                    if row.name == member:
                        content.extend(chunk)
                    remaining -= len(chunk)
            if row.name == member:
                found = bytes(content)
    if found is None:
        raise ValueError("Fallback archive has no build record")
    return found


def accepted(source, architecture):
    source = _source_directory(source, architecture)
    return _validate_report(source, read(source.parent / "results.json"), architecture)


def _source_directory(source, architecture):
    if architecture not in ("arm64", "amd64"):
        raise ValueError("Fallback assets require a supported native architecture")
    source = Path(source)
    if source.is_symlink() or source.name != "assets":
        raise ValueError("Fallback assets require an accepted platform run's assets directory")
    return source.resolve()


def _validate_report(source, raw, architecture):
    report = decode(raw)
    if (
        not isinstance(report, dict)
        or report.get("host_completed") is not True
        or report.get("passed") is not True
        or report.get("profile") != "release"
        or report.get("validation") not in ("ci", "distribution")
        or report.get("platform") != "linux-" + architecture
        or not isinstance(report.get("commit"), str)
        or re.fullmatch("[a-f0-9]{40}", report["commit"]) is None
        or report.get("failed_steps") != []
    ):
        raise ValueError("Fallback assets have no passing release-profile host acceptance")
    names = report.get("assets")
    actual = sorted(path.name for path in source.iterdir())
    if (
        not isinstance(names, list)
        or not all(isinstance(name, str) for name in names)
        or sorted(names) != actual
        or any(not path.is_file() or path.is_symlink() for path in source.iterdir())
    ):
        raise ValueError("Fallback consumed files differ from the accepted host file list")
    ledger = asset_set.load(source)
    target = ("aarch64" if architecture == "arm64" else "x86_64") + "-unknown-linux-gnu"
    if (
        ledger is None
        or set(ledger["builds"]) != {part + "/" + target for part in PARTS}
        or ledger["source"]["commit"] != report["commit"]
        or ledger["source"]["dirty"]
    ):
        raise ValueError("Fallback ledger does not bind five clean native parts to host acceptance")
    for part in PARTS:
        prefix = part + "-" + target
        if part == "hive-install":
            build_bytes = read(source / (prefix + ".build.json"))
        else:
            member = "share/" + component(part) + "/build.json"
            build_bytes = archive_build(source / (prefix + ".tar.gz"), member)
        build = decode(build_bytes)
        contract(build)
        if (
            build["part"] != part
            or build["target"] != target
            or build["source"] != ledger["source"]
            or bytes_record(build_bytes) != ledger["builds"][part + "/" + target]["build"]
        ):
            raise ValueError("A fallback part differs from its accepted source/build identity")
    return dict(
        source=str(source),
        commit=report["commit"],
        report_sha256=hashlib.sha256(raw).hexdigest(),
        checksums_sha256=hashlib.sha256(read(source / "SHA256SUMS", limit=65536)).hexdigest(),
        files=actual,
        platform=report["platform"],
        validation=report["validation"],
    )


def prepare_workflow(output, state, result):
    """Validate the actual copied CI workflow after VM cleanup, before publication."""
    unit = state["units"].get("suite-comparison")
    if unit is None or result["passed"] is not True or result["partial"] is not False:
        return None
    if (
        result["validation"] != "vm-full"
        or result["profile"] != "release"
        or result["host_completed"] is not True
        or result["cleanup"].get("confirmed") is not True
        or state["passed"] is not True
        or state["finish"] is None
        or unit.get("passed") is not True
        or type(unit.get("returncode")) is not int
        or unit.get("returncode") != 0
    ):
        raise ValueError("Native workflow publication requires completed VM acceptance and cleanup")
    attempt = unit.get("attempt")
    relative = unit.get("retained_logs")
    if type(attempt) is not int or attempt < 1 or relative != f"guest-suite-{attempt}":
        raise ValueError("Native workflow output is not its recorded suite directory")
    destination = Path(output) / relative
    if destination.is_symlink() or not destination.is_dir():
        raise ValueError("Native workflow output must be its retained directory")
    workflow = unit.get("workflow")
    if (
        not isinstance(workflow, dict)
        or workflow.get("validation") != "ci"
        or workflow.get("platform") != result["platform"]
        or not result["platform"].startswith("linux-")
        or workflow.get("commit") != result["commit"]
        or state["commit"] != result["commit"]
    ):
        raise ValueError("Native workflow source or platform differs from its completed VM")
    # Prebuilt parts already have their own source-bound acceptance; this guest
    # tested installation but did not build those parts itself.
    if workflow.get("asset_origin") == "prebuilt VM assets":
        return None
    raw = json_bytes(dict(workflow, host_completed=True))
    architecture = result["platform"].removeprefix("linux-")
    source = _source_directory(destination / "assets", architecture)
    _validate_report(source, raw, architecture)
    path = destination / "results.json"
    if path.is_symlink():
        raise ValueError("Native workflow acceptance cannot replace a symlink")
    if path.exists():
        if read(path) != raw:
            raise ValueError("Native workflow acceptance cannot replace an existing report")
        return None
    return path, raw
