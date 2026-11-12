"""Durable local CLI completion survives loss of the parent and its pipe."""

from pathlib import Path
import re
import uuid

from e2e.cache import cache_directory
from release.build_record import decode
from release.files import _read_private, atomic_write_json


def read(path):
    path = Path(path)
    if not path.is_absolute() or re.fullmatch("[a-f0-9]{32}\\.json", path.name) is None:
        raise ValueError("Invalid native completion identity")
    data = _read_private(path, 4096)
    if data is None:
        return None
    value = decode(data)
    expected = (
        {"format", "nonce", "kind", "returncode"}
        if isinstance(value, dict) and value.get("kind") == "completed"
        else {"format", "nonce", "kind"}
    )
    if (
        not isinstance(value, dict)
        or set(value) != expected
        or type(value["format"]) is not int
        or value["format"] != 1
        or value["nonce"] != path.stem
        or value["kind"] not in ("pending", "completed", "failed", "timeout", "uncertain")
        or value["kind"] == "completed"
        and (type(value["returncode"]) is not int or not -128 <= value["returncode"] <= 255)
    ):
        raise ValueError("Malformed native completion evidence")
    return value


def intent(output):
    directory = cache_directory(output / "clients")
    path = directory / (uuid.uuid4().hex + ".json")
    atomic_write_json(path, dict(format=1, nonce=path.stem, kind="pending"), create_once=True)
    return path


def confirmed(path):
    if path is None:
        return True
    value = read(path)
    return value is not None and value["kind"] in ("completed", "failed", "timeout")


def finish(path, value):
    if path is None:
        return
    old = read(path)
    if old is None or old["kind"] != "pending":
        raise ValueError("Native completion no longer matches its pending request")
    atomic_write_json(path, dict(format=1, nonce=Path(path).stem, **value))
