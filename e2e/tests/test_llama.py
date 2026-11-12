"""Server selection and the cached pinned build need no compiler, model or network."""

import json
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

from e2e.models import llama
from e2e.models import qwen3
from release import llama as release
from release.files import atomic_write_bytes, atomic_write_json, atomic_write_text

PIN = {
    "llama_cpp": {"repository": release.REPOSITORY, "commit": "c" * 40, "tree": "d" * 40},
    "model": {
        "repository": "Example/model",
        "revision": "e" * 40,
        "file": "model.gguf",
        "sha256": "f" * 64,
        "bytes": 5,
        "license": "Apache-2.0",
    },
}


class PinnedServer(unittest.TestCase):
    def setUp(self):
        patcher = patch.object(release, "load", return_value=PIN)
        patcher.start()
        self.addCleanup(patcher.stop)
        target = patch.object(llama, "native_target", return_value="aarch64-unknown-linux-gnu")
        target.start()
        self.addCleanup(target.stop)

    def test_darwin_cache_build_checks_the_optional_server_deployment_floor(self):
        def build(work, pin, target, environment):
            self.assertEqual(target, "aarch64-apple-darwin")
            executable = Path(work) / "build/bin" / release.TARGET
            executable.parent.mkdir(parents=True)
            header = bytearray(32)
            struct.pack_into("<II", header, 0, 0xFEEDFACF, 0x100000C)
            struct.pack_into("<II", header, 16, 1, 24)
            minimum = (13 << 16) | (3 << 8)
            command = struct.pack("<IIIIII", 0x32, 24, 1, minimum, 26 << 16, 0)
            atomic_write_bytes(executable, bytes(header) + command, mode=0o700)
            return executable, {"fixture": True}

        with (
            tempfile.TemporaryDirectory() as temporary,
            patch.object(llama, "native_target", return_value="aarch64-apple-darwin"),
            patch.object(release, "build", side_effect=build),
            patch.object(release, "version", return_value=(
                f"version: 0.6.0-dev (build 1, commit {PIN['llama_cpp']['commit'][:7]})"
            )),
            patch("release.common.run", return_value=(
                "server:\n /usr/lib/libSystem.B.dylib (compatibility version 1.0.0)"
            )),
        ):
            server = llama.build(Path(temporary) / "cache")
            record = json.loads(server.with_suffix(".json").read_text())
            self.assertEqual(record["dependencies"]["required_macos"], "13.3.0")
            self.assertTrue(llama.ready(server.parent.parent))

    def test_cache_build_drops_compiler_and_toolchain_overrides(self):
        environment = release.cache_environment(
            dict(
                PATH="/usr/bin",
                CXXFLAGS="-march=native",
                CMAKE_TOOLCHAIN_FILE="other.cmake",
                LD_PRELOAD="other.so",
                SOURCE_DATE_EPOCH="1",
                MACOSX_DEPLOYMENT_TARGET="15.0",
            ),
            system="Darwin",
        )
        for name in ("CXXFLAGS", "CMAKE_TOOLCHAIN_FILE", "LD_PRELOAD", "SOURCE_DATE_EPOCH"):
            self.assertNotIn(name, environment)
        self.assertEqual(environment["PATH"], "/usr/bin")
        self.assertEqual(environment["MACOSX_DEPLOYMENT_TARGET"], "13.3")
        self.assertEqual(environment["LC_ALL"], "C")

    def test_cached_executable_requires_its_matching_build_receipt(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = llama.cached(root)
            target.parent.mkdir()
            atomic_write_bytes(target, b"binary", mode=0o700)
            with patch.object(llama, "installed", return_value=None):
                with self.assertRaises(ValueError):
                    qwen3.executable(root)
                atomic_write_json(target.with_suffix(".json"), llama.receipt(target))
                self.assertEqual(qwen3.executable(root), target.resolve())
                atomic_write_bytes(target, b"changed", mode=0o700)
                with self.assertRaises(ValueError):
                    qwen3.executable(root)
                atomic_write_bytes(target, b"binary", mode=0o700)
                with patch.object(
                    release, "options", return_value={"GGML_NATIVE": "ON"}
                ), self.assertRaises(ValueError):
                    qwen3.executable(root)

    def test_installed_server_is_used_only_when_it_reports_the_pinned_commit(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary) / "bin"
            directory.mkdir()
            server = directory / llama.NAME
            scripts = (
                ("pinned", "printf 'version: 0.6.0-dev (build 1, commit "
                 f"{PIN['llama_cpp']['commit'][:7]})\\n' >&2", True),
                # A stale install after a pin bump falls back to the cached build.
                ("other commit", "printf 'version: 0.6.0-dev (build 1, commit 0000000)\\n' >&2", False),
                ("no version line", "printf 'llama-server\\n' >&2", False),
                ("failing --version", "exit 4", False),
            )
            for name, body, accepted in scripts:
                with self.subTest(name):
                    atomic_write_text(server, f"#!/bin/sh\n{body}\n", mode=0o700)
                    expected = server.resolve() if accepted else None
                    self.assertEqual(llama.installed(str(directory)), expected)
            self.assertIsNone(llama.installed(str(Path(temporary) / "absent")))

    def test_a_stale_installed_server_leaves_the_cached_build_in_use(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            directory = root / "bin"
            directory.mkdir()
            atomic_write_text(
                directory / llama.NAME,
                "#!/bin/sh\nprintf 'version: 0.6.0-dev (build 1, commit 0000000)\\n' >&2\n",
                mode=0o700,
            )
            target = llama.cached(root)
            target.parent.mkdir()
            atomic_write_bytes(target, b"binary", mode=0o700)
            atomic_write_json(target.with_suffix(".json"), llama.receipt(target))
            with patch.dict(os.environ, {"PATH": str(directory)}):
                self.assertEqual(qwen3.executable(root), target.resolve())

    def test_installed_server_takes_precedence_over_a_cached_build(self):
        with tempfile.TemporaryDirectory() as temporary:
            installed = Path(temporary) / "installed"
            atomic_write_bytes(installed, b"server", mode=0o700)
            with patch.object(llama, "installed", return_value=installed):
                self.assertEqual(qwen3.executable(Path(temporary)), installed.resolve())
            explicit = Path(temporary) / "explicit"
            atomic_write_bytes(explicit, b"server", mode=0o700)
            with patch.dict(os.environ, {"PATH": temporary}):
                self.assertEqual(qwen3.executable(Path(temporary), "explicit"), explicit.resolve())


if __name__ == "__main__":
    unittest.main()
