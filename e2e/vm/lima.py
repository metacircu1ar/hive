"""One exact Lima instance, durable request intent and checked stop/delete."""

from contextlib import contextmanager
import hashlib
import os
from pathlib import Path
import re

from e2e.cache import cache_directory
from e2e.campaign import cleanup_uncertain
from e2e.vm.images import GIB, config_bytes, configuration
from e2e.vm.folder import Folder, identity, owner as folder_owner
from e2e.vm.native import Native
from release.build_record import decode, read
from release.files import _read_private, atomic_write_bytes, atomic_write_json
from release.process import run
from release.progress import finalizer

GLOBALS = ("base.yaml", "default.yaml", "override.yaml")
OWNER_FIELDS = {
    "format",
    "instance_nonce",
    "name",
    "home",
    "home_identity",
    "ubuntu",
    "memory_gib",
    "source_commit",
    "mode",
    "image",
    "config",
    "config_sha256",
    "directory_identity",
    "config_identity",
    "create_completed",
    "create_settled",
    "deleted",
}


def globals_clean(home):
    for name in GLOBALS:
        if os.path.lexists(home / "_config" / name):
            raise ValueError("Refusing global Lima configuration in the validation home")


def socket_budget(home, name):
    path = home.resolve() / name / ("ssh.sock." + "0" * 16)
    if len(os.fsencode(path)) >= 104:
        raise ValueError("The VM home is too long for macOS SSH sockets")


def owner_state(output, home, *, nonce, version, memory_gib, commit, mode, image):
    if re.fullmatch("[a-f0-9]{32}", nonce) is None or re.fullmatch("[a-f0-9]{40}", commit) is None:
        raise ValueError("VM ownership requires complete nonce and source identities")
    name = "hive-" + nonce[:12]
    home = cache_directory(home).resolve()
    socket_budget(home, name)
    globals_clean(home)
    config = configuration(version, memory_gib, image, nonce)
    return dict(
        format=1,
        instance_nonce=nonce,
        name=name,
        home=str(home),
        home_identity=identity(home, directory=True),
        ubuntu=version,
        memory_gib=memory_gib,
        source_commit=commit,
        mode=mode,
        image=str(Path(image).resolve()),
        config=config,
        config_sha256=hashlib.sha256(config_bytes(config)).hexdigest(),
        directory_identity=None,
        config_identity=None,
        create_completed=False,
        create_settled=False,
        deleted=False,
    )


def validate_owner(value):
    if (
        not isinstance(value, dict)
        or set(value) != OWNER_FIELDS
        or type(value["format"]) is not int
        or value["format"] != 1
    ):
        raise ValueError("Unsupported VM owner record")
    nonce, home = value["instance_nonce"], value["home"]
    if (
        not isinstance(nonce, str)
        or re.fullmatch("[a-f0-9]{32}", nonce) is None
        or value["name"] != "hive-" + nonce[:12]
        or not isinstance(home, str)
        or not Path(home).is_absolute()
        or value["mode"] not in ("create", "probe", "essential", "full")
        or not isinstance(value["source_commit"], str)
        or re.fullmatch("[a-f0-9]{40}", value["source_commit"]) is None
        or not isinstance(value["image"], str)
        or not Path(value["image"]).is_absolute()
        or any(
            type(value[key]) is not bool
            for key in (
                "create_completed",
                "create_settled",
                "deleted",
            )
        )
    ):
        raise ValueError("Malformed VM owner identity")
    for key in ("home_identity", "directory_identity", "config_identity"):
        item = value[key]
        if item is None and key != "home_identity":
            continue
        if (
            not isinstance(item, list)
            or len(item) != 2
            or any(type(part) is not int or part < 0 for part in item)
        ):
            raise ValueError("Malformed VM inode identity")
    expected = configuration(value["ubuntu"], value["memory_gib"], value["image"], nonce)
    if (
        config_bytes(value["config"]) != config_bytes(expected)
        or value["config_sha256"] != hashlib.sha256(config_bytes(expected)).hexdigest()
    ):
        raise ValueError("VM owner configuration does not match its declared inputs")
    socket_budget(Path(home), value["name"])
    return value


class Lima:
    def __init__(self, admission, output, state, *, native=run):
        output = Path(output).absolute()
        identity(output, directory=True)
        self.admission, self.output = admission, output.resolve()
        self.state = validate_owner(state)
        self.home, self.name = Path(state["home"]), state["name"]
        self.native = native
        self.transport = Native(admission, self.home, native=native)
        self.env = self.transport.env
        self.cleanup = None
        self.check_home()

    @classmethod
    def load(cls, admission, output, *, native=run):
        output = Path(output).absolute()
        identity(output, directory=True)
        data = _read_private(output / "owner.json", 16384)
        if data is None:
            raise ValueError("No owned VM record exists in this output directory")
        return cls(admission, output, decode(data), native=native)

    def save(self):
        atomic_write_json(self.output / "owner.json", self.state)

    def owner(self):
        return folder_owner(self.home, self.name, resource="lima")

    def check_home(self):
        if identity(self.home, directory=True) != self.state["home_identity"]:
            raise ValueError("The recorded Lima home was replaced")

    def call(self, args, **options):
        self.check_home()
        with finalizer(self.check_home):
            return self.transport.call(args, **options)

    def request(self):
        return self.transport.request()

    def status(self):
        done = self.call(["list", "--all-fields", "--format", "json"], timeout=30)
        rows = [decode(line) for line in done.stdout.splitlines() if line.strip()]
        if any(not isinstance(row, dict) or not isinstance(row.get("name"), str) for row in rows):
            raise ValueError("Malformed Lima instance list")
        matching = [row for row in rows if row["name"] == self.name]
        if len(matching) > 1:
            raise ValueError("Ambiguous Lima instance list")
        return matching[0] if matching else None

    def inspect(self):
        row = self.status()
        if row is None:
            if os.path.lexists(self.home / self.name):
                raise ValueError("Native inspection omitted an existing VM directory")
            return None
        directory = self.home / self.name
        if row.get("dir") != str(directory) or row.get("status") not in ("Running", "Stopped"):
            raise ValueError("The owned VM has an unknown or broken native status")
        directory_id = identity(directory, directory=True)
        config = directory / "lima.yaml"
        config_id = identity(config)
        data = read(config, limit=65536)
        if hashlib.sha256(data).hexdigest() != self.state["config_sha256"] or data != config_bytes(
            self.state["config"]
        ):
            raise ValueError("Refusing a foreign or changed VM configuration")
        for key, current in (("directory_identity", directory_id), ("config_identity", config_id)):
            if self.state[key] is not None and self.state[key] != current:
                raise ValueError("An owned VM path was replaced")
        if (
            row.get("vmType") != "vz"
            or type(row.get("memory")) is not int
            or row["memory"] != self.state["memory_gib"] * GIB
            or row.get("cpus") != 4
        ):
            raise ValueError("Observed VM resources do not match the requested configuration")
        return row

    def begin(self, run_name):
        self.admission.begin("lima", run_name, self.owner())

    def create(self):
        globals_clean(self.home)
        if self.status() is not None or os.path.lexists(self.home / self.name):
            raise ValueError("Refusing to adopt an existing VM")
        generated = self.output / "lima.yaml"
        atomic_write_bytes(generated, config_bytes(self.state["config"]))
        self.save()
        self.admission.advance("creating")
        try:
            self.call(
                ["create", "--tty=false", "--name", self.name, str(generated)],
                timeout=1800,
                limit=64 * 1024**2,
            )
        except BaseException as error:
            if not cleanup_uncertain(error):
                self.state["create_settled"] = True
                with finalizer(self.save):
                    raise
            raise
        self.state.update(create_completed=True, create_settled=True)
        self.save()
        row = self.inspect()
        if row is None or row["status"] != "Stopped":
            raise ValueError("VM creation did not produce the owned stopped instance")
        self.state.update(
            directory_identity=identity(self.home / self.name, directory=True),
            config_identity=identity(self.home / self.name / "lima.yaml"),
        )
        self.save()
        self.admission.advance("created")
        return row

    def start(self):
        globals_clean(self.home)
        row = self.inspect()
        if row is None or self.state["deleted"] or not self.state["create_completed"]:
            raise ValueError("Only a completely created owned VM can start")
        if row["status"] == "Running":
            self.admission.advance("running")
            return row
        self.call(
            ["start", "--tty=false", "--timeout", "10m", self.name],
            phase="starting",
            timeout=900,
            limit=64 * 1024**2,
        )
        self.admission.advance("started")
        row = self.inspect()
        if row is None or row["status"] != "Running":
            raise ValueError("VM start did not reach the owned running state")
        self.admission.advance("running")
        return row

    def retire(self):
        active = self.admission.active
        if active is None:
            return
        if active["phase"] == "prepared" and self.transport.confirmed():
            self.cleanup = dict(confirmed=True, attempts=[])
        else:
            self.cleanup = Folder(self.admission, self.home, native=self.native).stop(self.name)
        if not self.cleanup["confirmed"] or not self.transport.confirmed():
            raise RuntimeError("VM campaign cleanup remains uncertain")
        self.admission.complete()

    @contextmanager
    def held(self, run_name):
        self.begin(run_name)
        with self.transport.protect():
            with finalizer(self.retire):
                with self.transport.protect():
                    yield self
