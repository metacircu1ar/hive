"""Synthetic frozen native parts for package and asset-set unit tests."""

import hashlib
import json
import shutil
import tempfile
from pathlib import Path
from unittest.mock import patch
from . import llama
from .common import ROOT, PARTS
from package_release import package
from .build_record import FEATURES
from .docs import product_files
from .frozen_inputs import record as input_record


# A complete synthetic pin: tests never depend on the shipped pin's verified IDs.
FAKE_PIN = {
    "llama_cpp": {"repository": llama.REPOSITORY, "commit": "c" * 40, "tree": "d" * 40},
    "model": {
        "repository": "Example/model",
        "revision": "e" * 40,
        "file": "model.gguf",
        "sha256": "f" * 64,
        "bytes": 5,
        "license": "Apache-2.0",
    },
}


def set_part(record, part):
    package, flags, names, variant = PARTS[part]
    previous = record["binaries"]
    record["binaries"] = {
        name: dict(previous.get(name, next(iter(previous.values())))) for name in names
    }
    record["part"] = part
    if part == llama.PART:
        record["command"] = list(llama.COMMAND)
        record["llama"] = {
            "repository": FAKE_PIN["llama_cpp"]["repository"],
            "commit": FAKE_PIN["llama_cpp"]["commit"],
            "tree": FAKE_PIN["llama_cpp"]["tree"],
            "cmake_options": llama.options(record["target"]),
            "cmake": "cmake version 3.28.3",
            "compiler": {"c": "fixture cc 1.0", "cxx": "fixture c++ 1.0"},
        }
        for binary in record["binaries"].values():
            binary.pop("cargo_profile", None)
            binary.pop("cargo_features", None)
            binary["version"] = (
                "version: 0.6.0-dev (build 1, commit "
                f"{FAKE_PIN['llama_cpp']['commit'][:7]})"
            )
        return
    record.pop("llama", None)
    record["command"] = [
        "cargo",
        "build",
        "--release",
        "--locked",
        "-p",
        package,
        *flags,
        "--message-format=json-render-diagnostics",
    ]
    for name, binary in record["binaries"].items():
        binary["cargo_profile"] = {"test": False, "opt_level": "3", "debug_assertions": False}
        binary["cargo_features"] = FEATURES[part]
        binary["version"] = (
            name + " " + binary["product_version"] + (f" ({variant})" if variant else "")
        )


class PackageFixture:
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.build = self.root / "freeze"
        (self.build / "release").mkdir(parents=True)
        self.output = self.root / "assets"
        deps = {"libraries": ["/usr/lib/libSystem.B.dylib"], "required_glibc": None}
        binaries = {}
        for name in ["hivectl", "hive-daemon"]:
            data = ("synthetic fixture " + name).encode()
            (self.build / "release" / name).write_bytes(data)
            binaries[name] = {
                "sha256": hashlib.sha256(data).hexdigest(),
                "bytes": len(data),
                "dependencies": deps,
                "product_version": "0.1.0",
            }
        notices = b"fixture third-party notices"
        (self.build / "THIRD_PARTY_LICENSES").write_bytes(notices)
        inputs = product_files(ROOT)
        inputs["packaging/install.sh"] = (ROOT / "packaging/install.sh").read_bytes()
        frozen = {}
        for name, data in inputs.items():
            path = self.build / "inputs" / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            frozen[name] = input_record(data, 0o755 if name == "packaging/install.sh" else 0o644)
        self.record = {
            "part": "hive",
            "target": "aarch64-apple-darwin",
            "profile": "release",
            "command": [
                "cargo",
                "build",
                "--release",
                "--locked",
                "-p",
                "hive",
                "--message-format=json-render-diagnostics",
            ],
            "binaries": binaries,
            "source": {"commit": "a" * 40, "dirty": False, "tree_sha256": "b" * 64},
            "inputs": frozen,
            "notices_sha256": hashlib.sha256(notices).hexdigest(),
            "notices_bytes": len(notices),
        }
        set_part(self.record, "hive")
        self.save()
        self.addCleanup(patch.stopall)
        patch("package_release.native_target", return_value=self.record["target"]).start()
        patch("package_release.inspect_binary", return_value=deps).start()
        patch(
            "package_release.run",
            side_effect=lambda args: json.loads(
                (Path(args[0]).parent.parent / "build.json").read_bytes()
            )["binaries"][Path(args[0]).name]["version"],
        ).start()
        patch("release.llama.load", return_value=FAKE_PIN).start()
        patch(
            "release.llama.version",
            side_effect=lambda path, **_: json.loads(
                (Path(path).parent.parent / "build.json").read_bytes()
            )["binaries"][Path(path).name]["version"],
        ).start()
        patch("release.asset_admission.native_target", return_value=self.record["target"]).start()
        patch("release.asset_admission.inspect_binary", return_value=deps).start()

    def save(self):
        (self.build / "build.json").write_text(json.dumps(self.record))

    def helper_build(self):
        set_part(self.record, "hive-install")
        (self.build / "release/hive-install").write_bytes(
            (self.build / "release/hivectl").read_bytes()
        )
        self.save()

    def part_build(self, part):
        destination = self.root / part
        shutil.copytree(self.build, destination)
        record = json.loads((destination / "build.json").read_bytes())
        previous = record["binaries"]
        set_part(record, part)
        for name in record["binaries"]:
            if name not in previous:
                shutil.copyfile(destination / "release/hivectl", destination / "release" / name)
        (destination / "build.json").write_text(json.dumps(record))
        return destination

    def all_parts(self):
        for part in PARTS:
            package(self.part_build(part), self.output)
