"""Prepare or serve pinned Qwen embeddings on Metal or an admitted Linux CPU.

The model pin is the product's shared pin; the server is the pinned static build.
"""

import argparse
import errno
import json
import os
from pathlib import Path
import shutil
import sys
import subprocess
import socket

from e2e.cache import cache_directory, fetch, model_pin, verify as verify_dependency
from e2e.embeddings.http import post, HTTPFailure
from e2e.paths import product_default
from e2e.models.memory import available as available_memory
from release.cancellation import deadline, pause
from release.files import atomic_write_json
from release.owned_process import spawn
from release.process import run, checked
from release.progress import phase

PIN = model_pin()
FILENAME = PIN["file"]
MIN_CPU_AVAILABLE_BYTES = 16 * 1024**3
HTTP_TIMEOUT_SECONDS = 120
READY_SECONDS = 180


def verify(path):
    verify_dependency(path, PIN)


def prepare(directory, port):
    directory = cache_directory(directory)
    url = f"https://huggingface.co/{PIN['repository']}/resolve/{PIN['revision']}/{FILENAME}"
    model = fetch(directory, FILENAME, url, PIN)
    config = json.loads(product_default("examples/qwen3-embedding.json").read_text())
    config["url"] = f"http://127.0.0.1:{port}/v1/embeddings"
    if sys.platform.startswith("linux"):
        config["timeout_seconds"] = HTTP_TIMEOUT_SECONDS
    atomic_write_json(directory / "comb-embedding.json", config)
    atomic_write_json(directory / "manifest.json", dict(PIN))
    return model


def admit_cpu_memory():
    available = available_memory()
    if available < MIN_CPU_AVAILABLE_BYTES:
        raise ValueError(
            "Qwen CPU serving requires at least 16 GiB available memory; increase the VM/container allocation (20 GiB or more) before retrying"
        )
    return available


def command(server, model, port, platform=None):
    platform = sys.platform if platform is None else platform
    slot_tokens = 8192
    if platform == "darwin":
        device = ["--device", "MTL0", "--n-gpu-layers", "all"]
    elif platform.startswith("linux"):
        device = ["--device", "none", "--n-gpu-layers", "0"]
        slot_tokens = 2048
    else:
        raise ValueError("Model serving supports macOS Metal or Linux CPU")
    return [
        str(server),
        "--model",
        str(model),
        "--alias",
        "qwen3-embedding-8b",
        "--embedding",
        "--pooling",
        "last",
        "--embd-normalize",
        "2",
        *device,
        "--flash-attn",
        "on",
        "--ctx-size",
        str(4 * slot_tokens),
        "--parallel",
        "4",
        "--no-kv-unified",
        "--batch-size",
        str(slot_tokens),
        "--ubatch-size",
        str(slot_tokens),
        "--cache-ram",
        "0",
        "--host",
        "127.0.0.1",
        "--port",
        str(port),
        "--no-webui",
        "--no-context-shift",
        "--metrics",
    ]


def executable(directory, selected=None):
    """An explicit server, else the installed comb-llama-server, else the cached build."""
    from .llama import cached, installed, ready

    if selected:
        found = shutil.which(str(selected))
    else:
        found = installed()
        if found is None and ready(directory):
            found = cached(directory)
    if found is None:
        raise ValueError(
            "llama-server is missing; run: python3 e2e/run.py prepare --tier embeddings --install"
        )
    return Path(found).resolve()


def require_idle_port(port):
    with socket.socket() as probe:
        probe.settimeout(1)
        status = probe.connect_ex(("127.0.0.1", port))
        if status != errno.ECONNREFUSED:
            raise OSError(status or errno.EADDRINUSE, "Cannot use the model server port")


def serve(directory, port, selected=None, *, platform=None):
    platform = sys.platform if platform is None else platform
    directory = cache_directory(directory)
    if platform.startswith("linux"):
        available = admit_cpu_memory()  # Before even checking/downloading model bytes.
        print(
            json.dumps(dict(device="cpu", available_memory_bytes=available, cpus=os.cpu_count())),
            flush=True,
        )
    require_idle_port(port)
    model = directory / FILENAME
    verify(model)
    server = executable(directory, selected)
    if platform == "darwin":
        devices = run([str(server), "--list-devices"], text=True, check=True)
        if "MTL0:" not in devices.stdout + devices.stderr:
            raise ValueError("Apple Metal device MTL0 is unavailable")
    run([str(server), "--version"], check=True)
    require_idle_port(port)
    with phase("models"):
        with spawn(
            command(server, model, port, platform=platform), limit=64 * 1024 * 1024
        ) as owner:
            with deadline(READY_SECONDS), phase("wait"):
                while True:
                    owner.pump()
                    if owner.exited():
                        status = owner.wait(timeout=1)
                        checked(
                            subprocess.CompletedProcess(
                                owner.command, status, bytes(owner.output), bytes(owner.errors)
                            )
                        )
                        raise RuntimeError("Model server stopped before readiness")
                    try:
                        health = post(f"http://127.0.0.1:{port}/health", None, timeout=1)
                    except HTTPFailure as error:
                        if error.status not in (0, 503):
                            raise
                    else:
                        if health == {"status": "ok"}:
                            break
                        raise ValueError("Unexpected model server health response")
                    pause(0.25)
            print(
                json.dumps(
                    dict(
                        ready=True,
                        port=port,
                        device="cpu" if platform.startswith("linux") else "MTL0",
                    )
                ),
                flush=True,
            )
            status = owner.wait(timeout=24 * 3600)
            checked(
                subprocess.CompletedProcess(
                    owner.command, status, bytes(owner.output), bytes(owner.errors)
                )
            )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("serve",))
    parser.add_argument("--directory", type=Path)
    parser.add_argument("--port", type=int, default=8081)
    parser.add_argument("--llama-server")
    args = parser.parse_args()
    if not 1 <= args.port <= 65535:
        parser.error("port must be 1..65535")
    directory = cache_directory(args.directory)
    serve(directory, args.port, args.llama_server)
