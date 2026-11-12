"""Serialize host campaigns and retain interrupted external-owner intent."""

import argparse
from contextlib import contextmanager
from functools import partial
import json
import os
from pathlib import Path
import pwd
import re
import uuid

from e2e.cache import cache_directory
from release.cancellation import check, cleanup, pause
from release.files import _read_private, atomic_write_json, record_lock
from release.progress import finalizer
from release.process import run

MAX_RECEIPT_BYTES = 16384


def default_root():
    return Path(pwd.getpwuid(os.getuid()).pw_dir) / ".hive-e2e-admission"


def read_receipt(root):
    data = _read_private(root / "campaign.json", MAX_RECEIPT_BYTES)
    if data is None:
        return None

    def unique(pairs):
        value = {}
        for name, item in pairs:
            if name in value:
                raise ValueError("Duplicate campaign receipt field")
            value[name] = item
        return value

    value = json.loads(data, object_pairs_hook=unique)
    if (
        not isinstance(value, dict)
        or set(value) != {"format", "active"}
        or type(value["format"]) is not int
        or value["format"] != 1
    ):
        raise ValueError("Unsupported campaign receipt")
    active = value["active"]
    if active is None:
        return None
    if (
        not isinstance(active, dict)
        or set(active) != {"kind", "nonce", "run", "phase", "owner"}
        or active["kind"] not in ("native", "docker", "lima")
        or not isinstance(active["nonce"], str)
        or re.fullmatch("[a-f0-9]{32}", active["nonce"]) is None
        or not isinstance(active["run"], str)
        or not 1 <= len(active["run"]) <= 128
        or not isinstance(active["phase"], str)
        or not 1 <= len(active["phase"]) <= 64
        or not isinstance(active["owner"], dict)
    ):
        raise ValueError("Malformed campaign ownership evidence")
    return active


def cleanup_uncertain(error):
    """Publication errors retain the native error as their explicit cause."""
    seen = set()
    while error is not None:
        if id(error) in seen or getattr(error, "owned_cleanup_uncertain", False):
            return True
        seen.add(id(error))
        error = error.__cause__
    return False


def recover(active, *, admission, native=run):
    if active["kind"] == "docker":
        from e2e.docker import recover as docker

        selected = partial(native, pass_fds=(admission.lock_fd,))
        return docker(active, native=selected)
    if active["kind"] == "lima":
        from e2e.vm.folder import recover as lima

        return lima(active, admission, native=native)
    return False


def describe_blocked(active):
    print(
        json.dumps(
            dict(
                blocked=True,
                kind=active["kind"],
                nonce=active["nonce"],
                recovery="python3 e2e/run.py campaign clear --nonce "
                + active["nonce"]
                + " --manual-cleanup-confirmed",
            )
        ),
        flush=True,
    )


class Admission:
    def __init__(self, root=None, recover=None):
        self.root = cache_directory(default_root() if root is None else root)
        self.active = None
        self.recover = recover
        self.lock_fd = None

    def save(self):
        atomic_write_json(self.root / "campaign.json", dict(format=1, active=self.active))

    def begin(self, kind, run, owner):
        if self.active is not None:
            raise RuntimeError("A campaign owner is already active")
        self.active = dict(
            kind=kind, nonce=uuid.uuid4().hex, run=run, phase="prepared", owner=owner
        )
        self.save()
        return self.active

    def advance(self, phase, **owner):
        if self.active is None:
            raise RuntimeError("No campaign owner is active")
        self.active["phase"] = phase
        self.active["owner"].update(owner)
        self.save()

    def complete(self):
        self.active = None
        self.save()

    @contextmanager
    def held(self, *, timeout=30):
        with record_lock(self.root / "admission", check=check, pause=pause, timeout=timeout) as fd:
            self.lock_fd = fd
            with finalizer(lambda: setattr(self, "lock_fd", None)):
                with self._admitted():
                    yield self

    @contextmanager
    def _admitted(self):
        self.active = read_receipt(self.root)
        if self.active is not None:
            with finalizer(
                lambda: describe_blocked(self.active) if self.active is not None else None
            ):
                with cleanup():
                    if (
                        self.active["kind"] != "native"
                        and self.recover is not None
                        and self.recover(self.active)
                    ):
                        self.complete()
            if self.active is not None:
                raise RuntimeError("Campaign cleanup is not confirmed")
        check()

        def finished():
            if self.active is not None:
                describe_blocked(self.active)
                raise RuntimeError("Campaign cleanup is not confirmed")

        with finalizer(finished):
            yield self


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("status", help="Show the active campaign receipt")
    clear = sub.add_parser("clear", help="Manually acknowledge all recorded activity is finished")
    clear.add_argument("--nonce", required=True)
    clear.add_argument("--manual-cleanup-confirmed", required=True, action="store_true")
    args = parser.parse_args()
    root = cache_directory(default_root())
    with record_lock(root / "admission", check=check, pause=pause):
        active = read_receipt(root)
        if args.command == "status":
            print(json.dumps(dict(active=active)), flush=True)
            return
        if active is None or active["nonce"] != args.nonce:
            raise ValueError("The campaign nonce does not match the active receipt")
        atomic_write_json(root / "campaign.json", dict(format=1, active=None))
        print(json.dumps(dict(cleared=args.nonce, manual_cleanup_confirmation=True)), flush=True)
