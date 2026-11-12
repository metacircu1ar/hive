"""Exact Ubuntu cloud image inputs, shared streamed cache and configuration."""

import json
import os
from pathlib import Path
import re
import sys

from e2e.cache import PINS, cache_directory, fetch, verify
from e2e.scripts.local_ci import native_architecture

VERSIONS = tuple(PINS["ubuntu"])
GIB = 1 << 30
MIN_FREE = 10 * GIB


def image_input(version, architecture=None):
    if version not in VERSIONS:
        raise ValueError("Choose a supported Ubuntu version")
    architecture = architecture or native_architecture()
    if architecture not in ("arm64", "amd64"):
        raise ValueError("The VM image requires a native arm64 or amd64 host")
    row = PINS["ubuntu"][version]
    name = "ubuntu-" + version + "-server-cloudimg-" + architecture + ".img"
    return dict(
        version=version,
        architecture=architecture,
        lima_arch="aarch64" if architecture == "arm64" else "x86_64",
        name=name,
        url="https://cloud-images.ubuntu.com/releases/"
        + row["codename"]
        + "/release-"
        + row["release"]
        + "/"
        + name,
        pin=dict(row[architecture]),
    )


def image_path(directory, version, *, install=False):
    item = image_input(version)
    directory = cache_directory(directory)
    path = directory / item["name"]
    if install:
        return fetch(directory, item["name"], item["url"], item["pin"])
    verify(path, item["pin"])
    return path


def configuration(version, memory_gib, image, nonce, *, architecture=None):
    if type(memory_gib) is not int or not 8 <= memory_gib <= 24:
        raise ValueError("VM memory must be 8..24 GiB")
    if not isinstance(nonce, str) or re.fullmatch("[a-f0-9]{32}", nonce) is None:
        raise ValueError("A VM configuration requires a complete owner nonce")
    item = image_input(version, architecture)
    image = Path(image).resolve()
    return dict(
        os="Linux",
        arch=item["lima_arch"],
        vmType="vz",
        plain=True,
        cpus=4,
        memory=str(memory_gib) + "GiB",
        disk="40GiB",
        images=[
            dict(
                location=str(image),
                arch=item["lima_arch"],
                digest="sha256:" + item["pin"]["sha256"],
            )
        ],
        mounts=[],
        portForwards=[],
        provision=[],
        probes=[],
        copyToHost=[],
        networks=[],
        containerd=dict(system=False, user=False),
        ssh=dict(loadDotSSHPubKeys=False, forwardAgent=False, forwardX11=False),
        message="Hive E2E owner: " + nonce,
    )


def config_bytes(value):
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()


def admit_host():
    if sys.platform != "darwin":
        raise ValueError("The native VZ validation tier requires macOS")
    if os.geteuid() == 0:
        raise ValueError("Native VM validation requires an ordinary user")
    native_architecture()
