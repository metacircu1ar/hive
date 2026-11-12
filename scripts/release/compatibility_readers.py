"""Read-only compatibility admission and newer-data refusal."""

import json
import os
from pathlib import Path
from .process import run as run_process

from .acceptance import json_command, require
from .common import digest
from .files import atomic_write_bytes, atomic_write_json
from .progress import finalizer


def identity(binary):
    if binary.name != "hive-daemon":
        return json_command([str(binary), "version", "--json"], env=os.environ)
    value = run_process(
        [str(binary), "--version"], text=True, timeout=10, check=True
    ).stdout.strip()
    words = value.split()
    require(len(words) >= 2 and words[0] == "hive-daemon", "Unexpected daemon identity")
    return {"product_version": words[1], "display": value}


def image(root):
    return {
        str(p.relative_to(root)): (p.stat().st_ino, p.stat().st_mode, digest(p))
        for p in root.rglob("*")
        if p.is_file()
    }


def refuse_newer(root, marker, command, env):
    original = marker.read_bytes()
    atomic_write_json(marker, {"format": 4294967295})
    before = image(root)
    with finalizer(lambda: atomic_write_bytes(marker, original)):
        result = run_process(
            command, capture_output=True, text=True, env=env, timeout=15, check=False
        )
        require(result.returncode != 0, "Older binary accepted newer data")
        require(
            json.loads(result.stdout)["error"]["code"] == "newer_format", "Wrong newer-data refusal"
        )
        require(image(root) == before, "Refusal changed owned data")


def check_previous_reader(root, marker, family, command, env):
    actual = json.loads(marker.read_bytes())["format"]
    supported = next(
        item for item in identity(Path(command[0]))["formats"] if item["name"] == family
    )
    before = image(root)
    result = run_process(command, capture_output=True, text=True, env=env, timeout=15, check=False)
    if actual > supported["maximum"]:
        require(
            result.returncode != 0 and json.loads(result.stdout)["error"]["code"] == "newer_format",
            "Previous reader failed newer-format refusal",
        )
    else:
        from .process import json_reply

        require(
            json_reply(result)["status"] == "current",
            "Previous reader cannot inspect advertised compatible data",
        )
    require(image(root) == before, "Previous reader changed candidate data")
