"""Retain notices from the exact normal dependency graph of a plain build."""

import json
import os
import re
from pathlib import Path
from .common import ROOT, digest, run

ALLOWED = {
    "MIT",
    "MIT-0",
    "MIT-CMU",
    "Apache-2.0",
    "LLVM-exception",
    "Unicode-3.0",
    "Zlib",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Unlicense",
    "0BSD",
    "BSL-1.0",
    "MPL-2.0",
    "CC0-1.0",
    "CDLA-Permissive-2.0",
    # llama.cpp's vendored SHA-1/SHA-256 notices dedicate these to the public
    # domain without the Unlicense or CC0 wording. Retain their exact notices.
    "LicenseRef-Public-Domain",
}
# Lines retained from a header that embeds its license after a reviewed marker.
EXCERPT_LINES = 64
FIRST_PARTY = {
    "hive": "Cargo.toml",
    "comb": "comb/Cargo.toml",
    "comb-protocol": "comb-protocol/Cargo.toml",
    "hive-install": "installer/Cargo.toml",
    "hive-setup-support": "setup-support/Cargo.toml",
    "hive-log": "logging/Cargo.toml",
    "hive-input": "input/Cargo.toml",
}


def first_party(entry):
    """A path dependency, even a workspace member, is not automatically our code."""
    relative = FIRST_PARTY.get(entry["name"])
    return (
        entry["source"] is None
        and relative is not None
        and Path(entry["manifest_path"]).resolve() == (ROOT / relative).resolve()
    )


def source_description(entry):
    source = entry["source"]
    if source == "registry+https://github.com/rust-lang/crates.io-index":
        return f"https://crates.io/api/v1/crates/{entry['name']}/{entry['version']}/download"
    return source or "path dependency from the build checkout"


def collect(graph, *, env, source):
    selected, metadata = graph
    validate_paths(metadata, source)
    records, text = [], []
    for entry in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if (entry["name"], entry["version"]) not in selected or first_party(entry):
            continue
        record, notice = package_notice(entry)
        records.append(record)
        text.extend(notice)
    record, notice = standard_notices(env=env)
    records.append(record)
    text.extend(notice)
    return records, "".join(text)


def dependency_metadata(package, flags, target, *, env, source):
    graph = run(
        [
            "cargo",
            "tree",
            "--locked",
            "-p",
            package,
            *flags,
            "--target",
            target,
            "--edges",
            "normal",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ],
        env=env,
    )
    selected = set(re.findall(r"^([\w-]+) v([^\s]+)", graph, re.M))
    metadata = json.loads(
        run(
            [
                "cargo",
                "metadata",
                "--locked",
                "--format-version",
                "1",
                "--filter-platform",
                target,
                *flags,
            ],
            env=env,
        )
    )
    available = {(p["name"], p["version"]) for p in metadata["packages"]}
    if selected - available:
        raise ValueError(f"Dependency metadata missing: {selected - available}")
    validate_paths(metadata, source)
    return selected, metadata


def validate_paths(metadata, source):
    """Build/dev-only and first-party path inputs need the same source binding."""
    for entry in metadata["packages"]:
        if entry["source"] is not None:
            continue
        source.check_path(entry["manifest_path"])
        if entry.get("license_file"):
            source.check_path(Path(entry["manifest_path"]).parent / entry["license_file"])
        for target in entry["targets"]:
            source.check_path(target["src_path"])
        if first_party(entry):
            source.check_path(ROOT / FIRST_PARTY[entry["name"]])
            continue
        for directory, children, files in os.walk(
            Path(entry["manifest_path"]).parent, onerror=walk_error
        ):
            if any((Path(directory) / name).is_symlink() for name in children):
                raise ValueError("Release dependency directories must not be symlinks")
            for name in files:
                source.check_path(Path(directory) / name)


def walk_error(error):
    raise error


def package_notice(entry):
    text = []
    license = entry.get("license") or ""
    tokens = set(re.findall(r"[A-Za-z0-9][A-Za-z0-9.+-]*", license)) - {"AND", "OR", "WITH"}
    if not tokens or not tokens <= ALLOWED:
        raise ValueError(f"Unreviewed dependency license: {entry['name']} {license}")
    directory = Path(entry["manifest_path"]).parent
    files = set()
    for path in directory.rglob("*"):
        if any(
            word in path.name.lower()
            for word in ("license", "licence", "copying", "copyright", "notice")
        ):
            files.update(p for p in (path.rglob("*") if path.is_dir() else [path]) if p.is_file())
    if entry.get("license_file"):
        files.add(directory / entry["license_file"])
    if not files:
        raise ValueError(f"Missing license text: {entry['name']} {entry['version']}")
    notices = []
    text.append(
        f"\n{'=' * 72}\n"
        f"{entry['name']} {entry['version']} — {license}\n"
        f"{entry.get('repository') or source_description(entry)}\n"
        f"Source: {source_description(entry)}\n"
    )
    for path in sorted(files):
        content = path.read_text()
        notices.append({"file": str(path.relative_to(directory)), "sha256": digest(path)})
        text.append(f"\n--- {path.relative_to(directory)} ---\n{content}\n")
    if entry["name"] == "libsqlite3-sys":
        # SQLite itself is public domain; its distributed source carries the declaration.
        source = directory / "sqlite3/sqlite3.c"
        sqlite = source.read_text()
        at = sqlite.index("The author disclaims copyright")
        declaration = sqlite[sqlite.rfind("/*", 0, at) : sqlite.index("*/", at) + 2] + "\n"
        text.append("\n--- Bundled SQLite declaration ---\n" + declaration)
    record = {
        "name": entry["name"],
        "version": entry["version"],
        "license": license,
        "notices": notices,
    }
    return record, text


def _license_tokens(license):
    tokens = set(re.findall(r"[A-Za-z0-9][A-Za-z0-9.+-]*", license)) - {"AND", "OR", "WITH"}
    return tokens if tokens and tokens <= ALLOWED else None


def _linked(root, path):
    """Notice files and their directories below the source root must not be links."""
    relative = path.relative_to(root)
    return any(
        (root / Path(*relative.parts[: i + 1])).is_symlink() for i in range(len(relative.parts))
    )


def source_notices(root, declarations, *, vendor, not_compiled, version, origin):
    """Notices of non-Cargo components compiled from one verified source tree.

    Each declaration names a component, its license expression and its notice
    files. A file paired with a marker contributes the bounded excerpt starting at
    the marker, for headers that embed their license. Every vendored directory must
    map to a declared component or be explicitly listed as not compiled.
    """
    root = Path(root)
    declared = {name for name, _, _ in declarations}
    vendored = root / "vendor"
    if vendored.is_dir() and not vendored.is_symlink():
        for entry in sorted(vendored.iterdir()):
            if entry.name in not_compiled:
                continue
            if vendor.get(entry.name) not in declared:
                raise ValueError(f"Unreviewed vendored component: vendor/{entry.name}")
    records, text = [], []
    for name, license, files in declarations:
        if _license_tokens(license) is None:
            raise ValueError(f"Unreviewed native component license: {name} {license}")
        text.append(
            f"\n{'=' * 72}\n{name} — {license}\n{origin}\nSource: {origin} at {version}\n"
        )
        notices = []
        for relative, marker in files:
            path = root / relative
            if _linked(root, path) or not path.is_file():
                raise ValueError(f"Missing license text: {name} {relative}")
            content = path.read_text()
            if marker is not None:
                lines = content.splitlines()
                start = next((i for i, line in enumerate(lines) if marker in line), None)
                if start is None:
                    raise ValueError(f"License marker not found: {name} {relative}")
                content = "\n".join(lines[start : start + EXCERPT_LINES]) + "\n"
            notices.append({"file": relative, "sha256": digest(path)})
            text.append(f"\n--- {relative} ---\n{content}\n")
        records.append({"name": name, "version": version, "license": license, "notices": notices})
    return records, "".join(text)


def standard_notices(*, env=None):
    text = []
    rust_docs = Path(run(["rustc", "--print", "sysroot"], env=env)) / "share/doc/rust"
    std_notices = []
    for name in ["COPYRIGHT-library.html", "licenses/MIT.txt", "licenses/Apache-2.0.txt"]:
        path = rust_docs / name
        if not path.is_file():
            raise ValueError(f"Missing toolchain redistribution notice: {path}")
        text.append(f"\n--- Rust standard library: {name} ---\n{path.read_text()}\n")
        std_notices.append({"file": name, "sha256": digest(path)})
    record = {
        "name": "rust-standard-library",
        "version": run(["rustc", "-V"], env=env),
        "license": "MIT OR Apache-2.0; see bundled exceptions",
        "notices": std_notices,
    }
    return record, text
