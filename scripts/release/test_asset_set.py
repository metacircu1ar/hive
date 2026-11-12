"""Coherent asset-set extension and pre-execution acceptance provenance."""

from contextlib import redirect_stdout
import io
import json
import shutil
import subprocess
import unittest
from unittest.mock import patch
import check_release_install
from package_release import package
from . import asset_admission, asset_set, common, files, llama
from .archive import json_bytes
from .build_record import bytes_record
from .frozen_inputs import record as input_record
from .test_package_fixture import PackageFixture, set_part


class AssetSetTests(PackageFixture, unittest.TestCase):
    def setUp(self):
        super().setUp()
        self.quiet = redirect_stdout(io.StringIO())
        self.quiet.__enter__()
        self.addCleanup(self.quiet.__exit__, None, None, None)

    def test_linux_dependency_aliases_require_same_system_files(self):
        root = self.root / "system"
        library = root / "usr/lib"
        library.mkdir(parents=True)
        (root / "lib").symlink_to(library, target_is_directory=True)
        (library / "libc.so.6").write_bytes(b"libc")
        (library / "libgcc_s.so.1").write_bytes(b"gcc")
        (library / "other").mkdir()
        shutil.copyfile(library / "libc.so.6", library / "other/libc.so.6")
        outside = self.root / "outside"
        outside.write_bytes(b"outside")
        (library / "escaped.so").symlink_to(outside)
        target = "aarch64-unknown-linux-gnu"
        recorded = {"libraries": [str(root / "lib/libc.so.6"), str(root / "lib/libgcc_s.so.1")],
                    "required_glibc": "2.34"}
        observed = {"libraries": [str(library / "libc.so.6"), str(library / "libgcc_s.so.1")],
                    "required_glibc": "2.34"}
        with patch.object(common, "LINUX_SYSTEM_LIBRARY_ROOTS", (library.resolve(),)):
            self.assertTrue(common.dependencies_match(recorded, observed, target))
            for bad in (
                {**observed, "libraries": observed["libraries"][::-1]},
                {**observed, "libraries": observed["libraries"] + [str(library / "libc.so.6")]},
                {**observed, "libraries": [str(library / "missing"), observed["libraries"][1]]},
                {**observed, "libraries": [str(outside), observed["libraries"][1]]},
                {**observed, "libraries": [str(library / "other/libc.so.6"),
                                            observed["libraries"][1]]},
                {**observed, "required_glibc": "2.35"},
            ):
                with self.subTest(bad=bad):
                    self.assertFalse(common.dependencies_match(recorded, bad, target))
            self.assertFalse(common.dependencies_match(recorded, observed, "aarch64-apple-darwin"))
            escaped = {"libraries": [str(library / "escaped.so")], "required_glibc": "2.34"}
            self.assertFalse(common.dependencies_match(escaped, escaped, target))

    def test_installed_verify_checks_real_dependency_comparator(self):
        prefix = self.root / "installed"
        (prefix / "bin").mkdir(parents=True)
        (prefix / "share/hive").mkdir(parents=True)
        library = self.root / "system/usr/lib"
        library.mkdir(parents=True)
        (self.root / "system/lib").symlink_to(library, target_is_directory=True)
        (library / "libc.so.6").write_bytes(b"libc")
        recorded = {"libraries": [str(self.root / "system/lib/libc.so.6")],
                    "required_glibc": "2.34"}
        observed = {"libraries": [str(library / "libc.so.6")], "required_glibc": "2.34"}
        record = json.loads(json.dumps(self.record))
        for name in ("hivectl", "hive-daemon"):
            shutil.copyfile(self.build / "release" / name, prefix / "bin" / name)
            record["binaries"][name]["dependencies"] = recorded
        (prefix / "share/hive/build.json").write_text(json.dumps(record))
        checker = object.__new__(check_release_install.InstalledChecks)
        checker.env, checker.identities = {}, {}
        with (patch.object(common, "LINUX_SYSTEM_LIBRARY_ROOTS", (library.resolve(),)),
              patch.object(check_release_install, "native_target",
                           return_value="aarch64-unknown-linux-gnu"),
              patch.object(check_release_install, "inspect_binary", return_value=observed),
              patch.object(check_release_install, "run_process",
                           return_value=subprocess.CompletedProcess([], 0, "hive (comb)\n", ""))):
            checker.verify(prefix, "hive")
            observed["libraries"] = [str(self.root / "outside")]
            with self.assertRaisesRegex(RuntimeError, "dependency closure"):
                checker.verify(prefix, "hive")

    def test_helper_admission_checks_dependency_comparator(self):
        self.all_parts()
        ledger = asset_set.load(self.output)
        target = "aarch64-apple-darwin"
        with patch.object(asset_admission, "dependencies_match",
                          wraps=common.dependencies_match) as compare:
            asset_admission.verify_helper(self.output, ledger, target)
            compare.assert_called_once()
        with patch.object(asset_admission, "inspect_binary",
                          return_value={"libraries": [], "required_glibc": None}):
            with self.assertRaisesRegex(ValueError, "provenance"):
                asset_admission.verify_helper(self.output, ledger, target)

    def test_installed_llama_uses_real_linux_dependency_alias_checks(self):
        prefix = self.root / "installed"
        (prefix / "bin").mkdir(parents=True)
        shutil.copyfile(self.build / "release/hivectl", prefix / "bin" / llama.BINARY)
        library = self.root / "system/usr/lib"
        library.mkdir(parents=True)
        (self.root / "system/lib").symlink_to(library, target_is_directory=True)
        (library / "libc.so.6").write_bytes(b"libc")
        record = json.loads(json.dumps(self.record))
        record["target"] = "aarch64-unknown-linux-gnu"
        set_part(record, llama.PART)
        record["binaries"][llama.BINARY]["dependencies"] = {
            "libraries": [str(self.root / "system/lib/libc.so.6")], "required_glibc": "2.34"
        }
        observed = {"libraries": [str(library / "libc.so.6")], "required_glibc": "2.34"}
        checker = object.__new__(check_release_install.InstalledChecks)
        checker.env, checker.identities = {}, {}
        with (
            patch.object(common, "LINUX_SYSTEM_LIBRARY_ROOTS", (library.resolve(),)),
            patch.object(check_release_install, "native_target", return_value=record["target"]),
            patch.object(check_release_install, "inspect_binary", return_value=observed),
            patch.object(
                llama, "version", return_value=record["binaries"][llama.BINARY]["version"]
            ),
        ):
            checker.verify_llama(prefix, record)
            observed["libraries"] = [str(self.root / "outside")]
            with self.assertRaisesRegex(RuntimeError, "dependency closure"):
                checker.verify_llama(prefix, record)

    def test_linux_helper_admission_accepts_only_same_file_aliases(self):
        library = self.root / "system/usr/lib"
        library.mkdir(parents=True)
        (self.root / "system/lib").symlink_to(library, target_is_directory=True)
        (library / "libc.so.6").write_bytes(b"libc")
        expected = {"libraries": [str(self.root / "system/lib/libc.so.6")],
                    "required_glibc": "2.34"}
        actual = {"libraries": [str(library / "libc.so.6")], "required_glibc": "2.34"}
        target = "aarch64-unknown-linux-gnu"
        name = "hive-install-" + target
        raw = b"synthetic build record"
        build = dict(part="hive-install", target=target, source={"commit": "test"},
                     inputs={"packaging/install.sh": {}},
                     binaries={"hive-install": dict(sha256="a", bytes=1, dependencies=expected)},
                     notices_sha256="b", notices_bytes=2)
        contribution = dict(build=bytes_record(raw), files={name: dict(sha256="a", bytes=1),
                            name + ".LICENSES": dict(sha256="b", bytes=2)})
        ledger = dict(builds={"hive-install/" + target: contribution}, source=build["source"],
                      product_version="0.1.0", bootstrap={},
                      inputs_sha256=bytes_record(json_bytes(build["inputs"]))["sha256"])
        with (patch.object(common, "LINUX_SYSTEM_LIBRARY_ROOTS", (library.resolve(),)),
              patch.object(asset_admission, "read", return_value=raw),
              patch.object(asset_admission, "decode", return_value=build),
              patch.object(asset_admission, "contract", return_value="0.1.0"),
              patch.object(asset_admission, "inspect_binary", return_value=actual)):
            asset_admission.verify_helper(self.output, ledger, target)
            actual["libraries"] = [str(library / "missing")]
            with self.assertRaisesRegex(ValueError, "provenance"):
                asset_admission.verify_helper(self.output, ledger, target)

    def test_different_source_or_bootstrap_refused_without_replacing_common_assets(self):
        package(self.build, self.output)
        before = {p.name: p.read_bytes() for p in self.output.iterdir()}
        other = self.part_build("comb")
        record = json.loads((other / "build.json").read_bytes())
        for kind in ("source", "bootstrap"):
            changed = json.loads(json.dumps(record))
            if kind == "source":
                changed["source"]["tree_sha256"] = "c" * 64
            else:
                data = b"changed bootstrap"
                (other / "inputs/packaging/install.sh").write_bytes(data)
                changed["inputs"]["packaging/install.sh"] = input_record(data, 0o755)
            (other / "build.json").write_text(json.dumps(changed))
            with self.subTest(kind=kind), self.assertRaisesRegex(ValueError, "Cannot mix"):
                package(other, self.output)
            self.assertEqual(before, {p.name: p.read_bytes() for p in self.output.iterdir()})

    def test_each_binary_profile_features_and_variant_are_admitted_independently(self):
        original = json.dumps(self.record)
        for field, value in (
            ("cargo_profile", dict(test=True, opt_level="3", debug_assertions=False)),
            ("cargo_profile", dict(test=False, opt_level="2", debug_assertions=False)),
            ("cargo_profile", dict(test=False, opt_level="3", debug_assertions=True)),
            ("cargo_features", ["barrier-delay", "comb", "default"]),
            ("version", "hivectl 0.1.0 (core)"),
        ):
            self.record = json.loads(original)
            self.record["binaries"]["hivectl"][field] = value
            self.save()
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                package(self.build, self.output)
            self.assertFalse(self.output.exists())

    def test_existing_unknown_or_mutated_outputs_are_not_extended(self):
        package(self.build, self.output)
        other = self.part_build("comb")
        archive = self.output / "hive-aarch64-apple-darwin.tar.gz"
        original = archive.read_bytes()
        archive.write_bytes(b"mutated")
        with self.assertRaisesRegex(ValueError, "committed ledger"):
            package(other, self.output)
        archive.write_bytes(original)
        (self.output / "unknown").write_bytes(b"unknown")
        with self.assertRaisesRegex(ValueError, "unknown members"):
            package(other, self.output)
        self.assertFalse((self.output / "comb-aarch64-apple-darwin.tar.gz").exists())

    def test_failed_commit_does_not_advertise_a_complete_set(self):
        package(self.build, self.output)
        other = self.part_build("comb")
        previous = (self.output / "SHA256SUMS").read_bytes()
        original_write = files.atomic_write_bytes

        def fail(path, *args, **kwargs):
            if path.name == asset_set.LEDGER:
                raise OSError("ledger fault")
            return original_write(path, *args, **kwargs)

        with (
            patch.object(files, "atomic_write_bytes", side_effect=fail),
            self.assertRaises(OSError),
        ):
            package(other, self.output)
        self.assertEqual((self.output / "SHA256SUMS").read_bytes(), previous)
        with self.assertRaisesRegex(ValueError, "unknown members"):
            asset_set.load(self.output)

    def test_mutation_after_publication_cannot_be_blessed_by_the_ledger(self):
        import package_release

        original = package_release.publish

        def mutate(*args):
            result = original(*args)
            for name in result:
                (self.output / name).write_bytes(b"changed after publication")
            return result

        with patch.object(package_release, "publish", side_effect=mutate):
            with self.assertRaisesRegex(ValueError, "consumed snapshot"):
                package(self.build, self.output)
        self.assertFalse((self.output / asset_set.LEDGER).exists())
        self.assertFalse((self.output / "SHA256SUMS").exists())

    def test_stale_checksum_index_refuses_acceptance_and_exact_retry_repairs_it(self):
        package(self.build, self.output)
        other = self.part_build("comb")
        previous = (self.output / "SHA256SUMS").read_bytes()
        original_write = files.atomic_write_text

        def fail(path, *args, **kwargs):
            if path.name == "SHA256SUMS":
                raise OSError("checksum fault")
            return original_write(path, *args, **kwargs)

        with patch.object(files, "atomic_write_text", side_effect=fail), self.assertRaises(OSError):
            package(other, self.output)
        self.assertEqual((self.output / "SHA256SUMS").read_bytes(), previous)
        with self.assertRaisesRegex(ValueError, "checksum index"):
            asset_set.load(self.output)
        package(other, self.output)
        ledger = asset_set.load(self.output)
        self.assertEqual(
            set(ledger["builds"]), {"hive/aarch64-apple-darwin", "comb/aarch64-apple-darwin"}
        )

    def test_incomplete_native_set_is_refused_before_any_installation(self):
        package(self.build, self.output)
        with self.assertRaisesRegex(ValueError, "all five"):
            asset_admission.admit(self.output, self.root / "snapshot")
        self.assertFalse((self.root / "snapshot").exists())

    def test_acceptance_rejects_replaced_helper_even_if_public_checksums_are_updated(self):
        self.all_parts()
        helper = self.output / "hive-install-aarch64-apple-darwin"
        helper.write_bytes(b"#!/bin/sh\nexit 99\n")
        lines = (self.output / "SHA256SUMS").read_text().splitlines()
        (self.output / "SHA256SUMS").write_text(
            "".join(
                (
                    bytes_record(helper.read_bytes())["sha256"]
                    if line.split()[1] == helper.name
                    else line.split()[0]
                )
                + "  "
                + line.split()[1]
                + "\n"
                for line in lines
            )
        )
        work = self.root / "work"
        work.mkdir()
        with patch(
            "release.owned_process.subprocess.Popen",
            side_effect=AssertionError("Execution before admission"),
        ) as execute:
            with self.assertRaisesRegex(ValueError, "committed ledger"):
                check_release_install.InstalledChecks(self.output, self.root / "installed", work)
            execute.assert_not_called()
        self.assertFalse((work / "mock-provider").exists())
        self.assertFalse((self.root / "installed").exists())

    def test_helper_build_must_match_profile_source_and_ledger_before_execution(self):
        self.all_parts()
        name = "hive-install-aarch64-apple-darwin.build.json"
        path = self.output / name
        original = path.read_bytes()
        ledger_original = (self.output / asset_set.LEDGER).read_bytes()
        for kind in ("profile", "source"):
            build = json.loads(original)
            if kind == "profile":
                build["binaries"]["hive-install"]["cargo_profile"]["test"] = True
            else:
                build["source"]["tree_sha256"] = "c" * 64
            data = json_bytes(build)
            path.write_bytes(data)
            ledger = json.loads(ledger_original)
            contribution = ledger["builds"]["hive-install/aarch64-apple-darwin"]
            contribution["build"] = bytes_record(data)
            contribution["files"][name] = input_record(data, 0o644)
            (self.output / asset_set.LEDGER).write_bytes(json_bytes(ledger))
            asset_set.commit_checksums(self.output, ledger)
            with self.subTest(kind=kind), self.assertRaises(ValueError):
                asset_admission.admit(self.output, self.root / ("snapshot-" + kind))

    def test_acceptance_uses_a_verified_private_bootstrap_snapshot(self):
        body = b"#!/bin/sh\nprintf verified\n"
        (self.build / "inputs/packaging/install.sh").write_bytes(body)
        self.record["inputs"]["packaging/install.sh"] = input_record(body, 0o755)
        self.save()
        self.all_parts()
        destination, helper = asset_admission.admit(self.output, self.root / "snapshot")
        self.assertEqual(helper["source"], self.record["source"])
        (self.output / "install.sh").write_bytes(b"#!/bin/sh\nprintf changed\n")
        self.assertEqual(
            subprocess.check_output(["sh", str(destination / "install.sh")]), b"verified"
        )
        self.assertEqual((destination / "install.sh").stat().st_mode & 0o777, 0o755)
        with self.assertRaisesRegex(ValueError, "committed ledger"):
            asset_admission.admit(self.output, self.root / "next")
        self.assertFalse((self.root / "next").exists())
