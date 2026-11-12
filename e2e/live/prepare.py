"""Discover live prerequisites without signing in, starting models or calling them."""

import json
import os
from pathlib import Path
import shutil
import subprocess
from types import SimpleNamespace
from urllib.parse import urlsplit, urlunsplit

from e2e.embeddings.http import HTTPFailure, post
from e2e.paths import product_root, require_paths
from release.process import ProcessFailure, run
from release.progress import phase

PACKAGES = {"codex": "@openai/codex", "claude": "@anthropic-ai/claude-code"}


def main(args, parser):
    root = product_root()
    missing, commands, providers = [], [], {}
    for package, binary, product in (("hive", "hivectl", "Hive"), ("comb", "comb", "Comb")):
        path = root / "target/release" / binary
        command = ["cargo", "build", "--release", "--locked", "-p", package]
        absent = (
            not path.is_file() or package == "hive" and not path.with_name("hive-daemon").is_file()
        )
        if absent and args.install:
            with phase("build"):
                run(command, cwd=root, timeout=3600, limit=64 * 1024**2, check=True)
            absent = (
                not path.is_file()
                or package == "hive"
                and not path.with_name("hive-daemon").is_file()
            )
        if absent:
            missing.append(package + " release binaries")
            commands.append(command)
        else:
            require_paths(SimpleNamespace(binary=path), parser, "binary", product=product)
    for provider, package in PACKAGES.items():
        path = shutil.which(provider)
        command = ["npm", "install", "--global", package]
        if path is None and args.install and shutil.which("npm") is not None:
            with phase("install"):
                run(command, timeout=1800, limit=64 * 1024**2, check=True)
            path = shutil.which(provider)
        providers[provider] = dict(available=path is not None)
        if path is None:
            missing.append(provider)
            commands.append(command)
    cache = (
        (
            args.cache
            or Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache") / "hive-e2e"
        )
        .expanduser()
        .resolve()
    )
    config = args.embedding_config or cache / "comb-embedding.json"
    if args.install:
        from e2e.prepare import main as prepare

        prepare(
            ["--tier", "embeddings", "--install", "--cache", str(cache), "--port", str(args.port)]
        )
    ready = False
    if config.is_file():
        value = json.loads(config.read_text())
        url = urlsplit(value["url"])
        if value["backend"] == "http" and url.hostname in ("localhost", "127.0.0.1", "::1"):
            health = urlunsplit((url.scheme, url.netloc, "/health", "", ""))
            try:
                ready = post(health, None, timeout=5).get("status") == "ok"
            except (HTTPFailure, ProcessFailure, OSError, ValueError, subprocess.SubprocessError):
                ready = False
    if not config.is_file():
        missing.append("Qwen embedding configuration")
        commands.append(
            [
                "python3",
                "e2e/run.py",
                "prepare",
                "--tier",
                "embeddings",
                "--install",
                "--cache",
                str(cache),
            ]
        )
    if not ready:
        missing.append("running local Qwen embedding server")
        if config.is_file():
            commands.append(
                [
                    "python3",
                    "e2e/run.py",
                    "prepare",
                    "--tier",
                    "embeddings",
                    "--install",
                    "--cache",
                    str(cache),
                ]
            )
        commands.append(["python3", "e2e/run.py", "model", "serve", "--directory", str(cache)])
    print(
        json.dumps(
            dict(
                ready=not missing,
                tier="live",
                providers=providers,
                missing=missing,
                commands=commands,
                embedding_config=str(config),
                logins="Requires existing codex-main and claude-main subscriptions; no login or auth check was attempted",
            )
        ),
        flush=True,
    )
    if missing:
        raise SystemExit(1)
