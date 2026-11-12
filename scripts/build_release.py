#!/usr/bin/env python3
"""Build and immediately freeze one plain package-scoped release variant."""

import argparse
import json
import subprocess
import time
from functools import partial
from pathlib import Path
from release.common import (
    PARTS,
    ROOT,
    digest,
    inspect_binary,
    native_target,
    run,
    write_json,
)
from release import llama
from release.build_environment import Environment, compiler_identity
from release.build_target import owned
from release.frozen_inputs import freeze
from release.source import Snapshot
from release.licenses import collect, dependency_metadata
from release.progress import entry, phase
from release.process import run as run_process
from release.cancellation import finish
from release.native_evidence import evidence
from release.files import atomic_copy, atomic_write_text, sync_directory, create_directory


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--part", choices=PARTS, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument(
        "--clean",
        action="store_true",
        help="Use a new disposable target directory for a clean-build measurement",
    )
    args = parser.parse_args()
    target = native_target()
    if args.output_dir.exists():
        parser.error("Output directory already exists; choose a fresh freeze directory")
    create_directory(args.output_dir)
    with evidence(args.output_dir / "diagnostics"):
        freeze_part(args, target)


def freeze_part(args, target):
    if args.part == llama.PART:
        return freeze_llama(args, target)
    package, flags, binaries, variant = PARTS[args.part]
    admitted = Environment(ROOT)
    with owned(ROOT, clean=args.clean) as build_target:
        env = admitted.for_target(build_target)
        execute = partial(run, env=env)
        before = Snapshot(ROOT, execute, exclude=(args.output_dir,))
        compiler_before = compiler_identity(execute(["rustc", "-Vv"]), target)
        admitted.remap_paths(env, execute(["rustc", "--print", "sysroot"]), compiler_before)
        with phase("licenses"):
            graph = dependency_metadata(package, flags, target, env=env, source=before)
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
        artifacts, elapsed = compile_artifacts(command, env, binaries, build_target)
        out = args.output_dir / "release"
        out.mkdir()
        records = {
            name: freeze_artifact(out / name, artifacts[name], target, variant, env)
            for name in binaries
        }
        inputs = freeze(ROOT, args.output_dir, before)
        with phase("licenses"):
            dependencies, notices = collect(graph, env=env, source=before)
        atomic_write_text(
            args.output_dir / "THIRD_PARTY_LICENSES", notices, mode=0o644, create_once=True
        )
        sync_directory(out)
        sync_directory(args.output_dir)
        sync_directory(args.output_dir.parent)
        compiler = compiler_identity(execute(["rustc", "-Vv"]), target)
        if compiler != compiler_before:
            raise ValueError("Compiler identity changed during build or dependency observation")
        before.check()
        admitted.config.check()
        build_record = {
            "part": args.part,
            "target": target,
            "profile": "release",
            "command": command,
            "source": before.public,
            "rustc": compiler,
            "clean": args.clean,
            "build_seconds": elapsed,
            "environment": admitted.public(),
            "inputs": inputs,
            "binaries": records,
            "dependencies": dependencies,
            "notices_sha256": digest(args.output_dir / "THIRD_PARTY_LICENSES"),
            "notices_bytes": (args.output_dir / "THIRD_PARTY_LICENSES").stat().st_size,
        }
    before.check()
    admitted.config.check()
    finish(
        lambda: (
            write_json(args.output_dir / "build.json", build_record),
            print(args.output_dir / "build.json"),
        )
    )


def freeze_llama(args, target):
    """Build the pinned static llama-server in a private tree removed afterwards."""
    admitted = Environment(ROOT, macos_deployment_target=llama.MACOS_DEPLOYMENT_TARGET)
    env = admitted.values
    execute = partial(run, env=env)
    pin = llama.load()
    # The private tree is a new owned temporary directory even without --clean.
    with owned(ROOT, clean=True) as work:
        before = Snapshot(ROOT, execute, exclude=(args.output_dir,))
        before.check_bytes(llama.PIN, (ROOT / llama.PIN).read_bytes())
        product_version = llama.product_version(before)
        began = time.monotonic()
        with phase("build"):
            executable, identity = llama.build(work, pin, target, env)
        elapsed = time.monotonic() - began
        out = args.output_dir / "release"
        out.mkdir()
        path = out / llama.BINARY
        atomic_copy(executable, path, mode=0o755, create_once=True)
        version = llama.version(path.resolve(), env=env)
        llama.check_version(version, pin["llama_cpp"]["commit"])
        binary = {
            "bytes": path.stat().st_size,
            "sha256": digest(path),
            "version": version,
            "product_version": product_version,
            "dependencies": inspect_binary(path, target, env=env),
        }
        inputs = freeze(ROOT, args.output_dir, before)
        with phase("licenses"):
            dependencies, notices = llama.notices(work / "source", pin, before)
        atomic_write_text(
            args.output_dir / "THIRD_PARTY_LICENSES", notices, mode=0o644, create_once=True
        )
        sync_directory(out)
        sync_directory(args.output_dir)
        sync_directory(args.output_dir.parent)
        before.check()
        admitted.config.check()
        build_record = {
            "part": args.part,
            "target": target,
            "profile": "release",
            "command": llama.COMMAND,
            "source": before.public,
            "llama": identity,
            "clean": True,
            "build_seconds": elapsed,
            "environment": admitted.public(),
            "inputs": inputs,
            "binaries": {llama.BINARY: binary},
            "dependencies": dependencies,
            "notices_sha256": digest(args.output_dir / "THIRD_PARTY_LICENSES"),
            "notices_bytes": (args.output_dir / "THIRD_PARTY_LICENSES").stat().st_size,
        }
    before.check()
    admitted.config.check()
    finish(
        lambda: (
            write_json(args.output_dir / "build.json", build_record),
            print(args.output_dir / "build.json"),
        )
    )


def compile_artifacts(command, env, binaries, build_target):
    began = time.monotonic()
    compiled = run_process(
        command,
        cwd=ROOT,
        env=env,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
        timeout=3600,
        limit=64 * 1024 * 1024,
    )
    artifacts = select_artifacts(compiled.stdout, binaries)
    if any(
        Path(artifacts[name]["executable"]).resolve() != build_target / "release" / name
        for name in binaries
    ):
        raise ValueError("Cargo artifact is outside the owned release target namespace")
    return artifacts, time.monotonic() - began


def select_artifacts(output, binaries):
    artifacts = {}
    for line in output.splitlines():
        frame = json.loads(line)
        if (
            frame.get("reason") == "compiler-artifact"
            and frame.get("executable")
            and frame["target"]["name"] in binaries
        ):
            if (
                frame["profile"]["test"]
                or frame["profile"]["opt_level"] != "3"
                or frame["profile"]["debug_assertions"]
            ):
                raise ValueError("Shipping binaries must use the normal optimized release profile")
            artifacts[frame["target"]["name"]] = frame
    if set(artifacts) != set(binaries):
        raise ValueError("Cargo did not identify every requested executable")
    return artifacts


def freeze_artifact(path, frame, target, variant, env):
    atomic_copy(frame["executable"], path, mode=0o755, create_once=True)
    version = run([str(path.resolve()), "--version"], env=env)
    fields = version.split()
    if len(fields) < 2:
        raise ValueError("Executable did not report a product version")
    product_version = fields[1]
    if variant and not version.endswith(f"({variant})"):
        raise ValueError(f"Unexpected compiled variant: {version}")
    return {
        "bytes": path.stat().st_size,
        "sha256": digest(path),
        "version": version,
        "product_version": product_version,
        "dependencies": inspect_binary(path, target, env=env),
        "cargo_features": frame["features"],
        "cargo_profile": frame["profile"],
    }


if __name__ == "__main__":
    raise SystemExit(entry(main, name="build"))
