"""Report/install Lima and the pinned image; never create or start a guest."""

import json
import shutil

from e2e.cache import cache_directory, cache_path, verify
from e2e.campaign import Admission, recover
from e2e.scripts.local_ci import staging
from e2e.vm import client
from e2e.vm.context import free_bytes
from e2e.vm.images import MIN_FREE, admit_host, image_input, image_path
from release.cancellation import finish
from release.process import run
from release.progress import phase


def main(args):
    admit_host()
    missing, commands = [], []
    installed = shutil.which("limactl") is not None
    if not installed and not args.install:
        print(
            json.dumps(dict(ready=False, missing=["limactl"], commands=["brew install lima"])),
            flush=True,
        )
        raise SystemExit(1)
    admission = Admission()
    admission.recover = lambda active: recover(active, admission=admission)
    with admission.held(), staging(admission, "prepare-vm"):
        if not installed:
            with phase("install"):
                run(
                    ["brew", "install", "lima"],
                    check=True,
                    timeout=1800,
                    pass_fds=(admission.lock_fd,),
                    limit=64 * 1024**2,
                )
            if shutil.which("limactl") is None:
                raise RuntimeError("Lima installation did not provide limactl")
        version = client.run(
            ["limactl", "--version"], lock_fd=admission.lock_fd, check=True, text=True
        ).stdout.strip()
        item = image_input(args.ubuntu)
        cache = cache_path(args.cache) if args.cache is not None else cache_path() / "vm-images"
        image = cache / item["name"]
        if args.install:
            needed = 0 if image.exists() else item["pin"]["bytes"]
            if free_bytes() < needed + MIN_FREE:
                raise ValueError("Pinned VM preparation needs image bytes plus 10 GiB free")
            image_path(cache, args.ubuntu, install=True)
        elif image.exists():
            cache_directory(cache)
            verify(image, item["pin"])
        else:
            missing.append(item["name"])
            commands.append(
                "python3 e2e/run.py prepare --tier vm --ubuntu " + args.ubuntu + " --install"
            )
    result = dict(
        ready=not missing,
        tier="vm",
        ubuntu=args.ubuntu,
        lima=version,
        image=str(image),
        image_sha256=item["pin"]["sha256"],
        missing=missing,
        commands=commands,
    )
    if missing:
        print(json.dumps(result), flush=True)
        raise SystemExit(1)
    finish(lambda: print(json.dumps(result), flush=True))
