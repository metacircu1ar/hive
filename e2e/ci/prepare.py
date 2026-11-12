"""Explicit Docker installation and readiness for the opt-in container tier."""

import json
import os
from pathlib import Path
import shutil
import sys

from e2e.campaign import Admission, recover
from e2e.ci.container import Container, image_inputs
from e2e.ci.steps import ci_env
from e2e.paths import product_root
from e2e.scripts.local_ci import native_architecture, selected_head, staging
from release.cancellation import finish
from release.process import ProcessFailure, run
from release.progress import finalizer, phase
from release.temporary import directory


def install_commands():
    if sys.platform == "darwin":
        return [["brew", "install", "--cask", "docker"]]
    if sys.platform.startswith("linux"):
        prefix = [] if os.geteuid() == 0 else ["sudo", "-n"]
        return [prefix + ["apt-get", "update"], prefix + ["apt-get", "install", "-y", "docker.io"]]
    raise ValueError("Docker preparation supports macOS and Linux")


def main(args):
    installation = install_commands()
    if shutil.which("docker") is None:
        if not args.install:
            print(
                json.dumps(
                    dict(
                        ready=False,
                        missing=["docker"],
                        commands=[" ".join(argv) for argv in installation],
                    )
                ),
                flush=True,
            )
            raise SystemExit(1)
        installer = Admission()
        with installer.held(), phase("install"):
            installer.begin("native", "install-docker", dict(pid=os.getpid()))
            environment = dict(
                ci_env(), DEBIAN_FRONTEND="noninteractive", HOMEBREW_NO_INTERACTIVE="1"
            )
            for command in installation:
                run(
                    command,
                    check=True,
                    timeout=1800,
                    limit=64 * 1024**2,
                    pass_fds=(installer.lock_fd,),
                    env=environment,
                )
            installer.complete()
        if shutil.which("docker") is None:
            raise RuntimeError("Docker installation did not provide its CLI")
    root = product_root()
    admission = Admission()

    def recover_previous(active):
        return recover(active, admission=admission)

    admission.recover = recover_previous
    ready = False
    try:
        with admission.held():
            with staging(admission, "prepare-container") as native:
                commit = selected_head(root, native)
                owner = Container.resolve(admission, None, native_architecture())
                inputs = image_inputs(root, commit, None, owner.architecture, native=native)
                ready = owner.lookup_image(inputs["image"], inputs["digest"]) is not None
            if args.install and not ready:
                with directory(prefix="hive-image-", parent=Path("/tmp")) as output:
                    owner.output = output
                    admission.begin("docker", "prepare-container", owner.docker.owner())
                    with finalizer(owner.retire):
                        owner.ensure_image(root, commit)
                    ready = True
    except ProcessFailure as error:
        print(
            json.dumps(
                dict(
                    ready=False,
                    missing=["container prerequisites"],
                    commands=["docker info", "git status"],
                )
            ),
            flush=True,
        )
        raise SystemExit(1) from error
    reply = dict(
        ready=ready,
        tier="container",
        architecture=native_architecture(),
        commands=[] if ready else ["python3 e2e/run.py prepare --tier container --install"],
    )
    if not ready:
        print(json.dumps(reply), flush=True)
        raise SystemExit(1)
    finish(lambda: print(json.dumps(reply), flush=True))
