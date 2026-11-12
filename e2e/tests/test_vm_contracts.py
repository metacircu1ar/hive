"""Pinned VM inputs, exact ownership and cleanup, without a real hypervisor."""

from contextlib import contextmanager
import copy
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from e2e.campaign import Admission
from e2e.vm import proof
from e2e.scripts import vm_validation as vm
from e2e.vm import checks, images, state
from e2e.vm.folder import Folder, delete as delete_vm
from e2e.vm.lima import GLOBALS, Lima, identity, owner_state, socket_budget, validate_owner
from release.files import atomic_write_bytes, atomic_write_json, create_directory
from release.process import checked, run as run_process


class BuildRecordSummary(unittest.TestCase):
    def test_real_guest_summary_accepts_cargo_and_cpp_build_records(self):
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory) / "build.json"
            for part in ["comb", "comb-llama"]:
                with self.subTest(part=part):
                    data = {
                        "build_seconds": 12.5,
                        "source": {"commit": "a" * 40, "dirty": False, "tree_sha256": "b" * 64},
                        "binaries": {"program": {"sha256": "c" * 64, "bytes": 123,
                            "version": "fixture", "dependencies": {"required_glibc": "2.35"}}},
                    }
                    if part == "comb":
                        data["rustc"] = "rustc fixture\nother metadata"
                    else:
                        data["llama"] = {"cmake": "cmake fixture", "compiler": {"cxx": "c++ fixture"}}
                    atomic_write_json(record, data)
                    result = run_process(
                        ["bash", "-eu", "-c", checks.BUILD_RECORD, "fixture", str(record)],
                        check=True, capture_output=True, text=True, timeout=10,
                    )
                    lines = result.stdout.splitlines()
                    self.assertEqual(len(lines), 1)
                    self.assertTrue(lines[0].startswith("HIVE_VM_JSON "))
                    summary = json.loads(lines[0].removeprefix("HIVE_VM_JSON "))
                    self.assertEqual(summary["build_seconds"], 12.5)
                    self.assertEqual(summary["binaries"]["program"]["required_glibc"], "2.35")


class Transport:
    def __init__(self, owner):
        self.owner, self.calls, self.row = owner, [], None
        self.stop_raises = False
        self.stop_sticks = False
        self.force_sticks = False
        self.stop_unknown = False
        self.start_returncode = 0

    def native(self, argv, **options):
        if argv[0] != "limactl":
            raise AssertionError("Unexpected fixture native executable")
        evidence = options.get("evidence")
        try:
            done = self.call(argv[1:], **options)
        except BaseException as error:
            proof.finish(
                evidence,
                dict(
                    kind="uncertain"
                    if getattr(error, "owned_cleanup_uncertain", False)
                    else "failed"
                ),
            )
            raise
        proof.finish(evidence, dict(kind="completed", returncode=done.returncode))
        if options.get("check"):
            checked(done)
        return done

    def call(self, argv, **options):
        self.calls.append(argv)
        actor = self.owner
        if argv[0] == "list":
            return subprocess.CompletedProcess(
                argv, 0, "" if self.row is None else json.dumps(self.row), ""
            )
        if argv[0] == "create":
            directory = actor.home / actor.name
            create_directory(directory, mode=0o700)
            atomic_write_bytes(directory / "lima.yaml", Path(argv[-1]).read_bytes())
            self.row = dict(
                name=actor.name,
                dir=str(directory),
                status="Stopped",
                vmType="vz",
                memory=actor.state["memory_gib"] * images.GIB,
                cpus=4,
                config=actor.state["config"],
            )
        elif argv[0] == "start":
            if self.start_returncode:
                return subprocess.CompletedProcess(argv, self.start_returncode, "", "")
            self.row["status"] = "Running"
        elif argv[0] == "stop":
            if (
                "--force" in argv
                and not self.force_sticks
                or "--force" not in argv
                and not self.stop_sticks
            ):
                self.row["status"] = "Stopped"
            if self.stop_raises and "--force" not in argv:
                error = TimeoutError("fixture stop failed before acknowledgement")
                if self.stop_unknown:
                    error.owned_cleanup_uncertain = True
                raise error
        elif argv[0] == "delete":
            shutil.rmtree(actor.home / actor.name)
            self.row = None
        else:
            raise AssertionError("Unknown fixture Lima call")
        return subprocess.CompletedProcess(argv, 0, "", "")


@contextmanager
def actor_fixture():
    # macOS's temporary path is too long for an SSH socket; test on a short owned
    # directory just like the production admission, never in a real LIMA_HOME.
    with tempfile.TemporaryDirectory(prefix="hv-", dir="/tmp") as temporary:
        root = Path(temporary).resolve()
        output = root / "out"
        create_directory(output, mode=0o700)
        admission = Admission(root / "admission")
        with admission.held():
            value = owner_state(
                output,
                root / "lima",
                nonce="a" * 32,
                version="24.04",
                memory_gib=8,
                commit="b" * 40,
                mode="full",
                image=root / "image.img",
            )
            actor = Lima(admission, output, value)
            transport = Transport(actor)
            with patch("e2e.vm.native.client.run", side_effect=transport.native):
                actor.begin("fixture-vm")
                try:
                    yield actor, transport
                finally:
                    # Unit fixtures explicitly own every simulated operation.
                    admission.complete()


class VmContracts(unittest.TestCase):
    def test_defaults_full_and_refused_options(self):
        args = vm.parse_args([])
        self.assertEqual((args.mode, args.ubuntu, args.memory_gib), ("essential", "24.04", 8))
        args = vm.parse_args(
            ["run", "--full", "--ubuntu", "22.04", "--memory-gib", "20", "--embedding-smoke"]
        )
        self.assertTrue(args.embedding_smoke)
        for argv in (
            ["run", "--memory-gib", "7"],
            ["run", "--resume"],
            ["run", "--through", "user"],
            ["run", "--embedding-smoke"],
            ["run", "--full", "--embedding-smoke"],
            ["v1"],
            ["probe", "--h2-check"],
        ):
            with self.subTest(argv=argv), self.assertRaises(SystemExit):
                vm.parse_args(argv)

    def test_all_image_pins_and_plain_no_mount_configuration(self):
        for version in images.VERSIONS:
            for architecture in ("arm64", "amd64"):
                item = images.image_input(version, architecture)
                self.assertTrue(item["url"].startswith("https://cloud-images.ubuntu.com/releases/"))
                self.assertEqual(len(item["pin"]["sha256"]), 64)
                self.assertGreater(item["pin"]["bytes"], 0)
                config = images.configuration(
                    version, 20, "/tmp/image", "a" * 32, architecture=architecture
                )
                self.assertEqual(config["arch"], item["lima_arch"])
                self.assertEqual(config["memory"], "20GiB")
                for key in (
                    "mounts",
                    "portForwards",
                    "provision",
                    "probes",
                    "copyToHost",
                    "networks",
                ):
                    self.assertEqual(config[key], [])
                self.assertTrue(config["plain"])
                self.assertEqual(
                    config["ssh"],
                    dict(loadDotSSHPubKeys=False, forwardAgent=False, forwardX11=False),
                )
                self.assertNotIn("param", config)
                self.assertEqual(config["message"], "Hive E2E owner: " + "a" * 32)
        with self.assertRaises(ValueError):
            socket_budget(Path("/tmp") / ("x" * 90), "hive-" + "a" * 12)

    def test_create_exact_argv_and_changed_config_resources_home_refuse(self):
        with actor_fixture() as (actor, transport):
            actor.create()
            create = next(argv for argv in transport.calls if argv[0] == "create")
            self.assertEqual(
                create,
                ["create", "--tty=false", "--name", actor.name, str(actor.output / "lima.yaml")],
            )
            self.assertEqual(
                (actor.home / actor.name / "lima.yaml").read_bytes(),
                (actor.output / "lima.yaml").read_bytes(),
            )
            config = actor.home / actor.name / "lima.yaml"
            original = config.read_bytes()
            atomic_write_bytes(config, original + b"\n")
            with self.assertRaises(ValueError):
                actor.inspect()
            atomic_write_bytes(config, original)
            # Publication changes the inode too: record the fixture's exact
            # restored file before testing a separate resource mismatch.
            actor.state["config_identity"] = identity(config)
            transport.row["memory"] += 1
            with self.assertRaises(ValueError):
                actor.inspect()
            transport.row["memory"] -= 1
            actor.home.rename(actor.home.with_name("old-home"))
            create_directory(actor.home, mode=0o700)
            with self.assertRaises(ValueError):
                actor.check_home()

    def test_global_inputs_refuse_before_create_start_and_resume_intent(self):
        for phase in ("create", "start", "resume"):
            for filename in GLOBALS:
                with (
                    self.subTest(phase=phase, filename=filename),
                    actor_fixture() as (actor, transport),
                ):
                    if phase != "create":
                        actor.create()
                    before = copy.deepcopy(actor.admission.active)
                    create_directory(actor.home / "_config", mode=0o700)
                    path = actor.home / "_config" / filename
                    atomic_write_bytes(path, b"{}\n")
                    transport.calls.clear()
                    with self.assertRaises(ValueError):
                        (actor.create if phase == "create" else actor.start)()
                    self.assertFalse(
                        any(argv[0] in ("create", "start") for argv in transport.calls)
                    )
                    self.assertEqual(actor.admission.active, before)
                    self.assertTrue(path.exists())
                    if phase == "create":
                        self.assertFalse((actor.home / actor.name).exists())
                    path.unlink()
                    (actor.create if phase == "create" else actor.start)()

    def test_raised_stop_still_forces_and_inspects_and_foreign_delete_refuses(self):
        with actor_fixture() as (actor, transport):
            actor.create()
            actor.start()
            transport.stop_raises = transport.stop_sticks = True
            with self.assertRaises(TimeoutError):
                Folder(actor.admission, actor.home).stop(actor.name)
            self.assertIn(["stop", "--force", actor.name], transport.calls)
            self.assertEqual(transport.row["status"], "Stopped")
            transport.row["dir"] = "/foreign"
            with self.assertRaises(ValueError):
                delete_vm(actor.admission, actor.name, home=actor.home)
            self.assertFalse(any(argv[0] == "delete" for argv in transport.calls))
            transport.row["dir"] = str(actor.home / actor.name)
            delete_vm(actor.admission, actor.name, home=actor.home)
            self.assertFalse((actor.home / actor.name).exists())

    def test_list_failure_and_missing_directory_never_mean_successful_absence(self):
        with actor_fixture() as (actor, transport):
            actor.create()
            transport.row = None
            with self.assertRaises(ValueError):
                actor.inspect()
            with patch.object(actor, "call", side_effect=TimeoutError("fixture list failed")):
                with self.assertRaises(TimeoutError):
                    actor.inspect()

    def test_owner_and_state_reject_unknown_fields_and_resume_binding_changes(self):
        with actor_fixture() as (actor, _):
            for key, value in (
                ("mode", "legacy"),
                ("format", 2),
                ("memory_gib", 30),
                ("extra", True),
            ):
                owner = copy.deepcopy(actor.state)
                owner[key] = value
                with self.assertRaises(ValueError):
                    validate_owner(owner)
            saved = state.fresh(actor.state, extras=["suite-comparison"])
            path = actor.output / "state.json"
            atomic_write_json(path, saved)
            self.assertEqual(state.load(path, actor.state, ["suite-comparison"]), saved)
            for key, value in (
                ("source_commit", "c" * 40),
                ("memory_gib", 20),
                ("mode", "essential"),
            ):
                owner = dict(actor.state, **{key: value})
                with self.assertRaises(ValueError):
                    state.load(path, owner, ["suite-comparison"])


if __name__ == "__main__":
    unittest.main()
