"""The build record binds actual frozen inputs and follows complete observation/cleanup."""

from contextlib import ExitStack, redirect_stdout
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import build_release
import package_release
from . import licenses
from .docs import GUIDES
from .test_build_source import Git
from .build_target import owned

TARGET = "aarch64-apple-darwin"
DEPENDENCIES = {"libraries": ["/usr/lib/libSystem.B.dylib"], "required_glibc": None}
COMPILER = (
    "rustc 1.98.1 (abcdef123 2026-09-01)\nbinary: rustc\ncommit-hash: "
    + "a" * 40
    + "\ncommit-date: 2026-09-01\nhost: aarch64-apple-darwin\nrelease: 1.98.1\n"
    "LLVM version: 22.1.2\n"
)


class BuildFreezeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        names = [
            *GUIDES,
            "logo.png",
            "comb/logo.png",
            "examples/qwen3-embedding.json",
            "packaging/install.sh",
            "Cargo.toml",
        ]
        for name in names:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("original input\n")
        self.git = Git(self.root, names)
        self.output = self.root / "dist/frozen"
        self.environment = None
        self.cargo_target = None
        self.change_during_notices = False
        self.change_compiler = False
        self.compiler = COMPILER
        self.uncertain_compile = False

    def execute(self, args, *, env, strip=True, text=True):
        if self.environment is None:
            self.environment = env
        self.assertIs(env, self.environment)
        if args[0] == "git":
            return self.git(args, strip=strip, text=text)
        if args[:2] == ["cargo", "tree"]:
            return "hive v0.1.0\n"
        if args[:2] == ["cargo", "metadata"]:
            return json.dumps(
                {
                    "packages": [
                        {
                            "name": "hive",
                            "version": "0.1.0",
                            "source": None,
                            "manifest_path": str(self.root / "Cargo.toml"),
                            "targets": [],
                        }
                    ]
                }
            )
        if args == ["rustc", "-Vv"]:
            return self.compiler
        if args == ["rustc", "--print", "sysroot"]:
            return str(self.root / "sysroot")
        self.assertEqual(args[-1], "--version")
        return Path(args[0]).name + " 0.1.0 (core)"

    def compile(self, command, **kwargs):
        self.assertIs(kwargs["env"], self.environment)
        self.environment = kwargs["env"]
        self.cargo_target = Path(self.environment["CARGO_TARGET_DIR"])
        self.assertIn(
            f"--remap-path-prefix={self.cargo_target}=/target",
            self.environment["CARGO_ENCODED_RUSTFLAGS"].split("\x1f"),
        )
        if self.uncertain_compile:
            error = OSError("synthetic uncertain Cargo cleanup")
            error.owned_cleanup_uncertain = True
            raise error
        self.assertNotEqual(self.cargo_target, self.root / "target")
        artifacts = []
        for name in ("hivectl", "hive-daemon"):
            binary = self.cargo_target / "release" / name
            binary.parent.mkdir(exist_ok=True)
            binary.write_bytes(("compiled " + name).encode())
            artifacts.append(
                {
                    "reason": "compiler-artifact",
                    "executable": str(binary),
                    "target": {"name": name},
                    "features": [],
                    "profile": {"test": False, "opt_level": "3", "debug_assertions": False},
                }
            )
        return subprocess.CompletedProcess(command, 0, "\n".join(map(json.dumps, artifacts)))

    def notices(self, graph, *, env, source):
        self.assertIs(env, self.environment)
        licenses.validate_paths(graph[1], source)
        script = (
            "import fcntl,os,sys\nfd=os.open(sys.argv[1],os.O_RDWR)\n"
            "try: fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB)\n"
            "except BlockingIOError: sys.exit(17)\n"
        )
        with subprocess.Popen(
            [sys.executable, "-c", script, str(self.cargo_target / ".publication.lock")]
        ) as process:
            self.assertEqual(process.wait(timeout=5), 17)
        if self.change_during_notices:
            (self.root / "README.md").write_text("changed during notices")
        if self.change_compiler:
            self.compiler = COMPILER.replace("1.98.1", "1.98.2")
        return [], "dependency notices\n"

    def build(self, *, clean=False):
        with ExitStack() as stack:
            stack.enter_context(
                patch.dict(os.environ, {"HOME": str(self.root), "PATH": os.defpath}, clear=True)
            )
            for name, value in (
                ("ROOT", self.root),
                ("run", self.execute),
                ("native_target", lambda: TARGET),
                ("collect", self.notices),
                ("inspect_binary", lambda *args, **kwargs: DEPENDENCIES),
            ):
                stack.enter_context(patch.object(build_release, name, value))
            stack.enter_context(patch.object(build_release, "run_process", self.compile))
            stack.enter_context(patch.object(licenses, "ROOT", self.root))
            stack.enter_context(patch.object(licenses, "run", self.execute))
            stack.enter_context(
                patch.object(
                    sys,
                    "argv",
                    [
                        "build_release.py",
                        "--part",
                        "hive-core",
                        "--output-dir",
                        str(self.output),
                        *(["--clean"] if clean else []),
                    ],
                )
            )
            stack.enter_context(redirect_stdout(io.StringIO()))
            build_release.main()

    def test_uncertain_clean_cargo_target_is_retained_through_the_build_path(self):
        self.uncertain_compile = True
        original = tempfile.mkdtemp
        with patch(
            "release.build_target.tempfile.mkdtemp",
            side_effect=lambda **kwargs: original(dir=self.root, **kwargs),
        ):
            with self.assertRaises(OSError) as caught:
                self.build(clean=True)
        self.assertTrue(self.cargo_target.exists())
        self.assertEqual(caught.exception.retained_root, str(self.cargo_target))
        self.assertFalse((self.output / "build.json").exists())

    def test_docs_and_bootstrap_remain_frozen_after_source_changes(self):
        self.build()
        record = json.loads((self.output / "build.json").read_bytes())
        self.assertNotIn(str(self.root), json.dumps(record))
        (self.root / "README.md").write_text("changed after completed freeze")
        (self.root / "packaging/install.sh").write_text("changed bootstrap")
        assets = self.root / "assets"
        with (
            patch.object(package_release, "native_target", return_value=TARGET),
            patch.object(package_release, "inspect_binary", return_value=DEPENDENCIES),
            patch.object(
                package_release,
                "run",
                side_effect=lambda args: Path(args[0]).name + " 0.1.0 (core)",
            ),
            redirect_stdout(io.StringIO()),
        ):
            package_release.package(self.output, assets)
        with tarfile.open(assets / f"hive-core-{TARGET}.tar.gz") as archive:
            self.assertEqual(
                archive.extractfile("share/hive/README.md").read(), b"original input\n"
            )
        self.assertEqual((assets / "install.sh").read_bytes(), b"original input\n")

    def test_late_license_phase_source_change_never_declares_a_build(self):
        self.change_during_notices = True
        with self.assertRaisesRegex(ValueError, "Source changed"):
            self.build()
        self.assertFalse((self.output / "build.json").exists())
        self.assertEqual((self.output / "inputs/README.md").read_bytes(), b"original input\n")

    def test_compiler_change_during_observation_never_declares_a_build(self):
        self.change_compiler = True
        with self.assertRaisesRegex(ValueError, "Compiler identity changed"):
            self.build()
        self.assertFalse((self.output / "build.json").exists())

    def test_cleanup_does_not_delete_a_replaced_temporary_target(self):
        original_mkdtemp = tempfile.mkdtemp
        with patch(
            "release.build_target.tempfile.mkdtemp",
            side_effect=lambda **kwargs: original_mkdtemp(dir=self.root, **kwargs),
        ):
            with self.assertRaisesRegex(ValueError, "target changed"):
                with owned(self.root, clean=True) as target:
                    retained = target.with_name("retained-original")
                    target.rename(retained)
                    target.mkdir(mode=0o700)
                    (target / "foreign").write_text("preserve replacement")
            self.assertEqual((target / "foreign").read_text(), "preserve replacement")
            self.assertTrue(retained.exists())
