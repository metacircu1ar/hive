"""Frozen native package publication and consumed-byte regressions."""

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tarfile
import unittest
from unittest.mock import patch
from package_release import package
from .common import ROOT
from .docs import GUIDES, validate_links
from .test_package_fixture import PackageFixture, set_part


class ReleasePackagingTests(PackageFixture, unittest.TestCase):
    def test_plain_record_archives_exact_files_with_notices_and_skill_reference(self):
        package(self.build, self.output)
        path = self.output / "hive-aarch64-apple-darwin.tar.gz"
        with tarfile.open(path) as archive:
            manifest = json.load(archive.extractfile("manifest.json"))
            self.assertEqual(manifest["format"], 1)
            self.assertEqual(manifest["product_version"], "0.1.0")
            self.assertEqual(set(archive.getnames()), set(manifest["files"]) | {"manifest.json"})
            self.assertIn("share/hive/docs/cli-reference.md", manifest["files"])
            self.assertIn("share/hive/skills/hive/SKILL.md", manifest["files"])
            self.assertIn("share/hive/THIRD_PARTY_LICENSES", manifest["files"])
        self.assertIn(
            hashlib.sha256(path.read_bytes()).hexdigest(), (self.output / "SHA256SUMS").read_text()
        )

    def test_changed_frozen_guide_during_archive_consumption_is_not_published(self):
        from release.archive import write_archive
        from release.files import PublicationError

        def changed(stream, payloads):
            guide = self.build / "inputs/README.md"
            data = guide.read_bytes()
            guide.write_bytes(b"X" + data[1:])
            write_archive(stream, payloads)

        with (
            patch("package_release.write_archive", changed),
            self.assertRaises(PublicationError) as caught,
        ):
            package(self.build, self.output)
        self.assertFalse(caught.exception.published)
        self.assertFalse((self.output / "hive-aarch64-apple-darwin.tar.gz").exists())

    def test_changed_frozen_bootstrap_is_not_copied_or_advertised(self):
        from release.files import PublicationError

        bootstrap = self.build / "inputs/packaging/install.sh"
        data = bootstrap.read_bytes()
        bootstrap.write_bytes(b"X" + data[1:])
        with self.assertRaises(PublicationError) as caught:
            package(self.build, self.output)
        self.assertFalse(caught.exception.published)
        self.assertFalse((self.output / "install.sh").exists())
        self.assertFalse((self.output / "SHA256SUMS").exists())

    def test_standalone_comb_carries_complete_product_guides(self):
        set_part(self.record, "comb")
        (self.build / "release/comb").write_bytes((self.build / "release/hivectl").read_bytes())
        self.save()
        package(self.build, self.output)
        with tarfile.open(self.output / "comb-aarch64-apple-darwin.tar.gz") as archive:
            prefix = "share/comb/"
            files = {
                n[len(prefix) :]: archive.extractfile(n).read()
                for n in archive.getnames()
                if n.startswith(prefix)
            }
        self.assertTrue(set(GUIDES).issubset(files))
        self.assertIn("examples/qwen3-embedding.json", files)
        self.assertIn("examples/comb.yaml", files)
        self.assertNotIn("AGENTS.md", files)
        self.assertFalse(
            any("benchmarks/results" in name or name.endswith("-plan.md") for name in files)
        )
        validate_links(files)

    def test_llama_server_part_carries_only_its_server_notices_and_record(self):
        set_part(self.record, "comb-llama")
        (self.build / "release/comb-llama-server").write_bytes(
            (self.build / "release/hivectl").read_bytes()
        )
        self.save()
        package(self.build, self.output)
        with tarfile.open(self.output / "comb-llama-aarch64-apple-darwin.tar.gz") as archive:
            manifest = json.load(archive.extractfile("manifest.json"))
            self.assertEqual(manifest["part"], "comb-llama")
            self.assertEqual(
                set(manifest["files"]),
                {
                    "bin/comb-llama-server",
                    "share/comb-llama/THIRD_PARTY_LICENSES",
                    "share/comb-llama/build.json",
                },
            )
        ledger = json.loads((self.output / "assets.json").read_text())
        self.assertIn("comb-llama/aarch64-apple-darwin", ledger["builds"])

    def test_llama_server_record_must_report_the_pinned_commit(self):
        set_part(self.record, "comb-llama")
        (self.build / "release/comb-llama-server").write_bytes(
            (self.build / "release/hivectl").read_bytes()
        )
        self.record["binaries"]["comb-llama-server"]["version"] = (
            "version: 0.6.0-dev (build 1, commit 0000000)"
        )
        self.save()
        with self.assertRaisesRegex(ValueError, "pinned commit"):
            package(self.build, self.output)
        self.assertFalse((self.output / "comb-llama-aarch64-apple-darwin.tar.gz").exists())
        set_part(self.record, "comb-llama")
        self.record["llama"]["cmake_options"]["GGML_NATIVE"] = "ON"
        self.save()
        with self.assertRaisesRegex(ValueError, "pinned llama-server contract"):
            package(self.build, self.output)

    def test_product_guides_reject_missing_links_and_internal_workspace(self):
        for body in [
            b"[missing](missing.md)",
            b'<img src="missing.png">',
            b"[reference]: missing.md",
        ]:
            with self.assertRaisesRegex(ValueError, "Unbundled documentation link"):
                validate_links({"README.md": body})
        for body in [
            b"[internal](https://github.com/metacircu1ar/parallax/blob/main/README.md)",
            b"[internal](../parallax/docs/README.md)",
        ]:
            with self.assertRaisesRegex(ValueError, "Internal workspace reference"):
                validate_links({"README.md": body})
        validate_links({"README.md": b"[local](guide.md#commands)", "guide.md": b"# Commands"})

    def test_changed_binary_or_notices_refused_before_output(self):
        (self.build / "release/hivectl").write_text("replaced by cargo test")
        with self.assertRaisesRegex(ValueError, "identity mismatch"):
            package(self.build, self.output)
        self.assertFalse(self.output.exists())

    def test_test_or_debug_build_record_is_not_a_shipping_build(self):
        for command in [
            ["cargo", "test", "--release", "--locked", "-p", "hive"],
            ["cargo", "build", "--locked", "-p", "hive"],
        ]:
            self.record["command"] = command
            self.save()
            with self.assertRaisesRegex(ValueError, "plain package-scoped release"):
                package(self.build, self.output)

    def test_missing_or_extra_binary_cannot_claim_complete_pair(self):
        del self.record["binaries"]["hive-daemon"]
        self.save()
        with self.assertRaisesRegex(ValueError, "wrong binary set"):
            package(self.build, self.output)

    def test_consumed_archive_and_helper_bytes_must_match_frozen_preflight(self):
        import package_release
        from release.files import PublicationError

        for helper in (False, True):
            if helper:
                self.helper_build()
            binary = "hive-install" if helper else "hivectl"
            source = self.build / "release" / binary
            original = source.read_bytes()
            real_publish = package_release.publish

            def mutate(*args):
                source.write_bytes(bytes([original[0] ^ 1]) + original[1:])
                return real_publish(*args)

            with (
                self.subTest(helper=helper),
                patch("package_release.publish", side_effect=mutate),
                self.assertRaisesRegex(PublicationError, "identity mismatch"),
            ):
                package(self.build, self.output)
            name = (
                "hive-install-aarch64-apple-darwin"
                if helper
                else "hive-aarch64-apple-darwin.tar.gz"
            )
            self.assertFalse((self.output / name).exists())
            self.assertFalse((self.output / "SHA256SUMS").exists())
            source.write_bytes(original)

    def test_changed_notice_after_preflight_cannot_be_advertised(self):
        import package_release
        from release.files import PublicationError

        notice = self.build / "THIRD_PARTY_LICENSES"
        original = notice.read_bytes()
        real_publish = package_release.publish

        def mutate(*args):
            notice.write_bytes(b"x" * len(original))
            return real_publish(*args)

        with (
            patch("package_release.publish", side_effect=mutate),
            self.assertRaisesRegex(PublicationError, "identity mismatch"),
        ):
            package(self.build, self.output)
        self.assertFalse((self.output / "hive-aarch64-apple-darwin.tar.gz").exists())
        self.assertFalse((self.output / "SHA256SUMS").exists())

    def test_failed_checksum_commit_can_retry_complete_assets(self):
        from release import files

        real_write = files.atomic_write_text

        def fail(path, *args, **kwargs):
            if path.name == "SHA256SUMS":
                raise OSError("checksum fault")
            return real_write(path, *args, **kwargs)

        with (
            patch.object(files, "atomic_write_text", side_effect=fail),
            self.assertRaisesRegex(OSError, "checksum fault"),
        ):
            package(self.build, self.output)
        archive = self.output / "hive-aarch64-apple-darwin.tar.gz"
        with tarfile.open(archive) as contents:
            self.assertIn("manifest.json", contents.getnames())
        self.assertFalse((self.output / "SHA256SUMS").exists())
        package(self.build, self.output)
        for line in (self.output / "SHA256SUMS").read_text().splitlines():
            digest, name = line.split()
            self.assertEqual(hashlib.sha256((self.output / name).read_bytes()).hexdigest(), digest)
        self.assertEqual((self.output / "install.sh").stat().st_mode & 0o777, 0o755)
        self.assertEqual(archive.stat().st_mode & 0o777, 0o644)
        self.assertEqual((self.output / "SHA256SUMS").stat().st_mode & 0o777, 0o644)

    def test_two_package_processes_preserve_checksum_union(self):
        other = self.root / "other"
        shutil.copytree(self.build, other)
        record = json.loads((other / "build.json").read_bytes())
        set_part(record, "comb")
        (other / "release/comb").write_bytes((other / "release/hivectl").read_bytes())
        (other / "build.json").write_text(json.dumps(record))
        script = """import json, sys
from pathlib import Path
import package_release as p
build, output = map(Path, sys.argv[1:])
record = json.loads((build/'build.json').read_bytes())
p.native_target = lambda: record['target']
p.inspect_binary = lambda *_: next(iter(record['binaries'].values()))['dependencies']
p.run = lambda args: record['binaries'][Path(args[0]).name]['version']
p.package(build, output)
"""
        env = dict(os.environ, PYTHONPATH=str(ROOT / "scripts"))
        processes = []
        try:
            processes.extend(
                subprocess.Popen(
                    [sys.executable, "-c", script, str(build), str(self.output)],
                    env=env,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                )
                for build in [self.build, other]
            )
            for process in processes:
                stdout, stderr = process.communicate(timeout=20)
                self.assertEqual(process.returncode, 0, (stdout, stderr))
        finally:
            for process in processes:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=5)
        sums = dict(
            (name, digest)
            for digest, name in (
                line.split() for line in (self.output / "SHA256SUMS").read_text().splitlines()
            )
        )
        for part in ["hive", "comb"]:
            name = f"{part}-aarch64-apple-darwin.tar.gz"
            self.assertEqual(
                sums[name], hashlib.sha256((self.output / name).read_bytes()).hexdigest()
            )
        self.assertFalse(list(self.output.glob(".hive-tmp-*")))
