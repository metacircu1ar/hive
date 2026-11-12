"""Report prerequisites; build/install/download only with an explicit --install."""

import argparse
import json
import os
from pathlib import Path
import shutil
import sys
from types import SimpleNamespace

from e2e.paths import product_root, require_paths
from release.process import run
from release.progress import phase


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--tier", choices=("mock", "embeddings", "container", "vm", "live"), default="mock"
    )
    parser.add_argument("--ubuntu", choices=("22.04", "24.04", "26.04"), default="24.04")
    parser.add_argument("--install", action="store_true")
    parser.add_argument("--cache", type=Path)
    parser.add_argument("--embedding-config", type=Path)
    parser.add_argument("--port", type=int, default=8081)
    args = parser.parse_args(argv)
    if not 1 <= args.port <= 65535:
        parser.error("port must be 1..65535")
    if args.tier == "live":
        from e2e.live.prepare import main as live

        live(args, parser)
        return
    if args.tier == "container":
        from e2e.ci.prepare import main as container

        container(args)
        return
    if args.tier == "vm":
        from e2e.vm.prepare import main as vm

        vm(args)
        return
    root = product_root()
    names = ("hivectl", "hive-daemon") if args.tier == "mock" else ("comb",)
    package = "hive" if args.tier == "mock" else "comb"
    missing = [name for name in names if not (root / "target/release" / name).is_file()]
    commands = [f"cargo build --release --locked -p {package}"] if missing else []
    if missing and args.install:
        with phase("build"):
            run(
                ["cargo", "build", "--release", "--locked", "-p", package],
                cwd=root,
                timeout=3600,
                check=True,
                limit=64 * 1024**2,
            )
        missing = [name for name in missing if not (root / "target/release" / name).is_file()]
    if not missing:
        binary = root / "target/release" / ("hivectl" if package == "hive" else "comb")
        require_paths(
            SimpleNamespace(binary=binary),
            parser,
            "binary",
            product="Hive" if package == "hive" else "Comb",
        )
    if args.tier == "embeddings":
        from e2e.cache import cache_directory
        from e2e.models import llama, qwen3

        cache = (
            args.cache
            or Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "hive-e2e"
        )
        cache = cache.expanduser().absolute()
        if args.install or cache.exists():
            cache_directory(cache)
        # An installed comb-llama-server needs no build; otherwise build the same pin.
        server = llama.installed()
        if sys.platform == "darwin":
            prerequisites = ("curl",) if server else ("curl", "cmake", "git", "c++")
            installation = ["brew", "install", "cmake"]
        elif sys.platform.startswith("linux"):
            prerequisites = ("curl",) if server else ("curl", "cmake", "git", "c++", "make")
            installation = ["apt-get", "install", "-y", "build-essential", "cmake", "git", "curl"]
        else:
            parser.error("Local models support macOS Metal or Linux CPU")
        absent = [name for name in prerequisites if shutil.which(name) is None]
        if absent and args.install and (sys.platform == "darwin" or os.geteuid() == 0):
            if sys.platform.startswith("linux"):
                run(
                    ["apt-get", "update"],
                    check=True,
                    timeout=600,
                    limit=32 * 1024**2,
                    env=dict(os.environ, DEBIAN_FRONTEND="noninteractive"),
                )
            run(
                installation,
                check=True,
                timeout=1800,
                limit=64 * 1024**2,
                env=dict(os.environ, DEBIAN_FRONTEND="noninteractive"),
            )
            absent = [name for name in prerequisites if shutil.which(name) is None]
        if absent:
            missing.extend(absent)
            commands.append(" ".join(installation))
        if server is None:
            if args.install and not absent:
                llama.build(cache)
            if not cache.exists() or not llama.ready(cache):
                missing.append("pinned llama-server")
                commands.append("python3 e2e/run.py prepare --tier embeddings --install")
        if sys.platform.startswith("linux"):
            try:
                qwen3.admit_cpu_memory()
            except ValueError as error:
                print(
                    json.dumps(
                        dict(
                            ready=False,
                            missing=missing,
                            message=str(error),
                            commands=commands,
                            required_available_gib=16,
                        )
                    )
                )
                raise SystemExit(1) from None
        model = cache / qwen3.FILENAME
        if args.install and not absent:
            qwen3.prepare(cache, args.port)
        elif model.exists():
            qwen3.verify(model)
        else:
            missing.append(qwen3.FILENAME)
            commands.append("python3 e2e/run.py prepare --tier embeddings --install")
    if missing:
        print(json.dumps(dict(ready=False, missing=missing, commands=commands)))
        raise SystemExit(1)
    print(json.dumps(dict(ready=True, tier=args.tier)))
