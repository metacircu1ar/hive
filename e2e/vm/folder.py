"""Cleanup authority is the reserved private Lima home and each VM's saved marker."""

import os
from pathlib import Path
import pwd
import re
import shlex
import stat

from e2e.vm.native import Native
from release.build_record import decode, read
from release.process import checked, run
from release.progress import finalizer

MARKER = re.compile(r"Hive E2E owner: [a-f0-9]{32}")
NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,63}")


def default_home():
    return Path(pwd.getpwuid(os.getuid()).pw_dir) / ".hive-vm.noindex"


def safe(path, *, directory=False, private=False):
    info = Path(path).lstat()
    if (
        not (stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode))
        or info.st_uid != os.getuid()
        or info.st_mode & (0o077 if private else 0o022)
        or not directory
        and info.st_nlink != 1
    ):
        raise ValueError("VM access requires an owned directory or regular file, without symlinks")
    return info


def identity(path, *, directory=False):
    info = safe(path, directory=directory, private=directory)
    return [info.st_dev, info.st_ino]


class Folder:
    def __init__(self, admission=None, home=None, *, native=run):
        raw = Path(default_home() if home is None else home).absolute()
        self.home = raw.parent.resolve() / raw.name
        self.native = Native(admission, self.home, native=native)
        self.admission = admission

    def path(self, name):
        if not isinstance(name, str) or NAME.fullmatch(name) is None:
            raise ValueError("Use one valid Lima instance name")
        return self.home / name

    def marker(self, name):
        safe(self.home, directory=True, private=True)
        directory = self.path(name)
        safe(directory, directory=True)
        config = directory / "lima.yaml"
        safe(config)
        value = decode(read(config, limit=65536))
        marker = value.get("message") if isinstance(value, dict) else None
        if not isinstance(marker, str) or MARKER.fullmatch(marker) is None:
            raise ValueError("Refusing cleanup of a VM without the saved Hive marker")
        return marker

    def rows(self):
        if not os.path.lexists(self.home):
            return {}
        safe(self.home, directory=True, private=True)
        done = self.native.call(["list", "--all-fields", "--format", "json"], timeout=30)
        rows = {}
        for line in done.stdout.splitlines():
            if not line.strip():
                continue
            row = decode(line)
            if (
                not isinstance(row, dict)
                or not isinstance(row.get("name"), str)
                or NAME.fullmatch(row["name"]) is None
                or row["name"] in rows
                or row.get("dir") != str(self.path(row["name"]))
                or not isinstance(row.get("status"), str)
                or not 0 <= len(row["status"]) <= 64
            ):
                raise ValueError("Malformed or out-of-folder Lima instance listing")
            rows[row["name"]] = row
        return rows

    def listing(self):
        rows = self.rows()
        if not self.home.exists():
            return []
        # Lima reserves underscore-prefixed directories for shared metadata.
        names = set(rows) | {
            path.name
            for path in self.home.iterdir()
            if not path.name.startswith(("_", ".")) and (path.is_dir() or path.is_symlink())
        }
        found = []
        for name in sorted(names):
            try:
                self.marker(name)
            except (OSError, ValueError):
                owned = False
            else:
                owned = True
            found.append(
                dict(
                    name=name,
                    state=rows.get(name, {}).get("status", "Unregistered") or "Unknown",
                    owned=owned,
                    delete_command=shlex.join(["python3", "e2e/run.py", "vm", "delete", name]),
                    attention=(
                        "If deletion cannot prove Stopped, inspect this VM manually before confirming cleanup"
                        if owned and rows.get(name, {}).get("status") not in ("Running", "Stopped")
                        else None
                        if owned
                        else "Saved marker is absent or unreadable; manual attention is required"
                    ),
                )
            )
        return found

    def inspect(self, name):
        rows = self.rows()
        row = rows.get(name)
        if row is None and not os.path.lexists(self.path(name)):
            return None
        self.marker(name)
        return row if row is not None else dict(name=name, status="Unregistered")

    def stop(self, name):
        if not self.native.confirmed():
            return dict(confirmed=False, attempts=[])
        row = self.inspect(name)
        starting = self.admission.active["phase"] == "starting"
        if starting:
            # Even a nonzero CLI exit can precede delayed PID publication.
            # Local completion alone cannot settle a dispatched failed start.
            pid = row.get("hostAgentPID", 0) if row is not None else 0
            if type(pid) is not int or pid <= 0:
                return dict(confirmed=False, attempts=[])
            self.admission.advance("stopping")
        if row is None:
            return dict(confirmed=True, attempts=[])
        result = dict(confirmed=False, attempts=[])

        def force_and_verify():
            current = self.inspect(name)
            if current is not None and (
                current["status"] != "Stopped" or starting and row["status"] == "Stopped"
            ):
                stopped = self.native.call(["stop", "--force", name], check=False, timeout=60)
                result["attempts"].append(
                    dict(operation="force-stop", returncode=stopped.returncode)
                )
                current = self.inspect(name)
                if current is not None and current["status"] != "Stopped":
                    checked(stopped)
                    raise RuntimeError("The marked VM is still not stopped after forced cleanup")
            result["confirmed"] = True

        with finalizer(force_and_verify):
            if row["status"] == "Running":
                stopped = self.native.call(["stop", "--tty=false", name], check=False, timeout=300)
                result["attempts"].append(dict(operation="stop", returncode=stopped.returncode))
        return result

    def delete(self, name):
        result = self.stop(name)
        if not result["confirmed"]:
            raise RuntimeError("Pending local work prevents proving VM cleanup")
        row = self.inspect(name)
        if row is not None:
            if row["status"] != "Stopped":
                raise RuntimeError("VM deletion requires verified Stopped state")
            self.admission.advance("deleting")
            self.native.call(["delete", "--tty=false", name], timeout=300)
        if self.inspect(name) is not None or os.path.lexists(self.path(name)):
            raise RuntimeError("VM deletion did not prove absence")
        return dict(result, deleted=True, name=name)


def owner(home, name, *, resource="lima-cleanup"):
    return dict(
        resource=resource,
        home=str(home),
        name=name,
        native_uncertain=False,
        client_file=None,
    )


def matching(active, home, name):
    return (
        active is not None
        and active["kind"] == "lima"
        and active["owner"].get("resource") in ("lima", "lima-cleanup")
        and active["owner"].get("home") == str(home)
        and active["owner"].get("name") == name
    )


def delete(admission, name, *, home=None, native=run):
    folder = Folder(admission, home, native=native)
    folder.path(name)
    if admission.active is None:
        folder.marker(name)
        admission.begin("lima", "vm-delete", owner(folder.home, name))
    elif not matching(admission.active, folder.home, name):
        raise RuntimeError("Another campaign record prevents VM deletion")
    with folder.native.protect():
        if not folder.native.confirmed():
            raise RuntimeError("The matching record still has unproved local work")
        admission.advance(admission.active["phase"], resource="lima-cleanup")
        result = folder.delete(name)
        admission.complete()
        return result


def recover(active, admission, *, native=run):
    if active["kind"] != "lima":
        return False
    item = active["owner"]
    home, name = Path(item["home"]), item["name"]
    if not home.is_absolute() or item.get("resource") not in ("lima", "lima-cleanup"):
        raise ValueError("Malformed active VM cleanup request")
    folder = Folder(admission, native=native)
    if home != folder.home:
        raise ValueError("Cleanup is confined to Hive's reserved private Lima folder")
    folder.path(name)
    if not folder.native.confirmed():
        return False
    with folder.native.protect():
        if item["resource"] == "lima-cleanup":
            return folder.delete(name)["confirmed"]
        if active["phase"] == "prepared":
            # No mutating VM request was dispatched before its preconditions.
            return True
        return folder.stop(name)["confirmed"]
