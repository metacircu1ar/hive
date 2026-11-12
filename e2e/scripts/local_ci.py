"""Opt-in native/container checks; the default is a small native-architecture Linux run."""

import argparse
from contextlib import contextmanager
from functools import partial
import json
import os
from pathlib import Path
import platform
import pwd
import re
import shutil
import sys
import time
import uuid

from e2e.cache import cache_directory
from e2e.campaign import Admission, cleanup_uncertain, recover
from e2e.ci.container import Container
from e2e.ci.runner import Runner
from e2e.paths import product_root
from release.cancellation import bounded, check, deadline, finish
from release.files import atomic_stream, atomic_write_json, create_directory
from release.native_evidence import evidence
from release.process import run
from release.progress import finalizer, phase
from release.temporary import directory

PLATFORMS = ("macos", "linux-amd64", "linux-arm64")
ESSENTIAL_SECONDS = 3600
FULL_SECONDS = 24 * 3600


def native_architecture():
    machine = platform.machine().lower()
    if machine in ("aarch64", "arm64"):
        return "arm64"
    if machine in ("x86_64", "amd64"):
        return "amd64"
    raise ValueError("Container validation supports arm64 and amd64")


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", nargs="+", choices=PLATFORMS)
    parser.add_argument("--full", action="store_true", help="Run the full workflow matrix")
    parser.add_argument("--variant", action="append", help="Run a named workflow variant")
    parser.add_argument("--distribution-only", action="store_true")
    parser.add_argument("--fresh", action="store_true", help="Clear only owned build caches")
    parser.add_argument(
        "--output-dir", type=Path, help="New runs root; default is dist/e2e/ci-runs"
    )
    parser.add_argument("--run-id", help="Share this ID between separately invoked platforms")
    parser.add_argument("--inside", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--parent-nonce", help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    args.platform = args.platform or ["linux-" + native_architecture()]
    if len(set(args.platform)) != len(args.platform):
        parser.error("Select each platform once")
    if args.distribution_only and (args.variant or args.full):
        parser.error("--distribution-only is separate from --full and --variant")
    if not args.inside and "macos" in args.platform and sys.platform != "darwin":
        parser.error("The macos platform requires a native Mac")
    if args.run_id is not None and re.fullmatch("[A-Za-z0-9_-]{1,64}", args.run_id) is None:
        parser.error("--run-id must be 1..64 letters, digits, underscores or hyphens")
    return args


def selected_head(root, native):
    value = native(["git", "rev-parse", "HEAD"], cwd=root, check=True, text=True).stdout.strip()
    if re.fullmatch("[a-f0-9]{40}", value) is None:
        raise ValueError("Cannot establish the selected source commit")
    return value


def bundle_head(root, output, commit, native):
    destination = output / "head.bundle"

    def write(stream):
        native(
            ["git", "bundle", "create", "-", "HEAD"],
            cwd=root,
            stdout=stream,
            check=True,
            timeout=120,
            limit=4 * 1024**3,
        )

    atomic_stream(destination, write, create_once=True)
    heads = native(
        ["git", "bundle", "list-heads", str(destination)], check=True, text=True
    ).stdout.splitlines()
    if heads != [commit + " HEAD"]:
        raise ValueError("The source commit changed while its bundle was prepared")
    return destination


def link_target(checkout, target):
    create_directory(target, exist_ok=True)
    (checkout / "target").symlink_to(target, target_is_directory=True)


def runner(platform_name, checkout, output, args, *, inherited=()):
    return Runner(
        platform_name,
        checkout,
        output,
        selected=args.variant,
        full=args.full,
        distribution=args.distribution_only,
        inherited=inherited,
    )


def native_macos(root, args, output, admission):
    admission.begin("native", output.name, dict(pid=os.getpid()))
    native = partial(run, pass_fds=(admission.lock_fd,))
    commit = selected_head(root, native)
    bundle = bundle_head(root, output, commit, native)
    work_root = cache_directory(Path(pwd.getpwuid(os.getuid()).pw_dir) / ".hive-e2e.noindex")
    cache = cache_directory(work_root / "cache-macos")
    if args.fresh:
        shutil.rmtree(cache)
        cache_directory(cache)
    with directory(prefix="ci-", parent=work_root) as work:
        checkout = work / "src"
        native(["git", "clone", "-q", str(bundle), str(checkout)], check=True, timeout=120)
        link_target(checkout, cache / "target")
        result = runner("macos", checkout, output, args, inherited=(admission.lock_fd,)).run()
        if result["commit"] != commit:
            raise ValueError("Native validation changed its source identity")
        atomic_write_json(output / "native-result.json", result)
    admission.complete()
    return result


def validate_inside(args):
    if (
        not sys.platform.startswith("linux")
        or not Path("/.dockerenv").is_file()
        or args.output_dir != Path("/out")
        or len(args.platform) != 1
        or not args.platform[0].startswith("linux-")
        or args.parent_nonce is None
    ):
        raise ValueError("The inside entry requires an owned Linux container")
    parent = json.loads(Path("/out/parent.json").read_text())
    if (
        not isinstance(parent, dict)
        or set(parent) != {"nonce", "commit", "platform"}
        or parent["nonce"] != args.parent_nonce
        or parent["platform"] != args.platform[0]
    ):
        raise ValueError("Container parent marker does not match this invocation")
    return parent


def publish(results):
    for output, result in results:
        result["host_completed"] = True
        atomic_write_json(output / "results.json", result)
    print(json.dumps(dict(summary=[result for _, result in results], accepted=True)), flush=True)
    if not all(result["passed"] for _, result in results):
        raise SystemExit(1)


def inside(args):
    parent = validate_inside(args)
    checkout = Path("/work/src")
    link_target(checkout, Path("/work/target"))
    result = runner(args.platform[0], checkout, args.output_dir, args).run()
    if result["commit"] != parent["commit"]:
        raise ValueError("Container source does not match its parent")

    def provisional():
        atomic_write_json(args.output_dir / "container-result.json", result)
        if not result["passed"]:
            raise SystemExit(1)

    finish(provisional)


@contextmanager
def staging(admission, run_name):
    """Read-only Git/Docker probes have only canonical, directly owned groups."""
    admission.begin("native", run_name, dict(pid=os.getpid()))
    try:
        yield partial(run, pass_fds=(admission.lock_fd,))
    except BaseException as error:
        if not cleanup_uncertain(error):
            with finalizer(admission.complete):
                raise
        raise
    else:
        admission.complete()


def container(root, args, output, platform_name, admission):
    with staging(admission, output.name) as native:
        commit = selected_head(root, native)
        bundle_head(root, output, commit, native)
        owner = Container.resolve(admission, output, platform_name.split("-")[1])
    admission.begin("docker", output.name, owner.docker.owner())
    with finalizer(owner.retire):
        image = owner.ensure_image(root, commit)
        identifier = owner.create(image, fresh=args.fresh, commit=commit, platform=platform_name)
        command = [
            "container",
            "exec",
            identifier,
            "python3",
            "/work/src/e2e/run.py",
            "container",
            "--inside",
            "--platform",
            platform_name,
            "--output-dir",
            "/out",
            "--parent-nonce",
            admission.active["nonce"],
        ]
        if args.full:
            command.append("--full")
        for variant in args.variant or []:
            command += ["--variant", variant]
        if args.distribution_only:
            command.append("--distribution-only")
        admission.advance("executing")
        done = owner.docker.call(
            command, check=False, timeout=bounded(FULL_SECONDS), limit=64 * 1024**2
        )
        result = json.loads((output / "container-result.json").read_text())
        if (
            not isinstance(result, dict)
            or result.get("platform") != platform_name
            or result.get("commit") != commit
            or type(result.get("passed")) is not bool
            or done.returncode != (0 if result["passed"] else 1)
        ):
            raise ValueError("Container result does not match its native exit or source")
    result["container_cleanup"] = owner.cleanup
    return result


def main():
    args = parse_args()
    if args.inside:
        inside(args)
        return
    root = product_root()
    run_id = args.run_id or time.strftime("%Y%m%d-%H%M%S") + "-" + uuid.uuid4().hex[:12]
    outputs = (args.output_dir or root / "dist/e2e/ci-runs").expanduser().resolve()
    if args.output_dir is not None and outputs.exists():
        raise ValueError("Use a new output directory")
    create_directory(outputs, exist_ok=True, mode=0o700)
    results = []
    admission = Admission()

    def recover_previous(active):
        return recover(active, admission=admission)

    admission.recover = recover_previous
    with admission.held():
        for selected in args.platform:
            check()
            output = outputs / (selected + "-" + run_id)
            create_directory(output, mode=0o700)
            seconds = (
                FULL_SECONDS
                if args.full or args.variant or args.distribution_only
                else ESSENTIAL_SECONDS
            )
            with deadline(seconds), phase("verify"), evidence(output):
                result = (
                    native_macos(root, args, output, admission)
                    if selected == "macos"
                    else container(root, args, output, selected, admission)
                )
            results.append((output, result))
    finish(partial(publish, results))
