"""Vendored dependency notices must survive the normal-graph selection."""

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from . import licenses
from .source import Snapshot
from .test_build_source import Git


class LicenseTests(unittest.TestCase):
    def test_path_dependency_is_included_but_exact_first_party_is_excluded(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            vendor = root / "vendor/widget"
            vendor.mkdir(parents=True)
            (root / "Cargo.toml").write_text("workspace fixture")
            (vendor / "Cargo.toml").write_text("vendored manifest fixture")
            (vendor / "src").mkdir()
            (vendor / "src/lib.rs").write_text("pub fn widget() {}")
            (vendor / "LICENSE").write_text("Third-party widget permission notice.\n")
            docs = root / "toolchain/share/doc/rust"
            (docs / "licenses").mkdir(parents=True)
            for name in ["COPYRIGHT-library.html", "licenses/MIT.txt", "licenses/Apache-2.0.txt"]:
                (docs / name).write_text("Toolchain notice.\n")
            entries = [
                dict(
                    name="hive",
                    version="0.1.0",
                    source=None,
                    manifest_path=str(root / "Cargo.toml"),
                    targets=[],
                ),
                dict(
                    name="widget",
                    version="1.0.0",
                    source=None,
                    license="MIT",
                    manifest_path=str(vendor / "Cargo.toml"),
                    targets=[{"src_path": str(vendor / "src/lib.rs")}],
                ),
            ]

            environment = {"CARGO_BUILD_JOBS": "4", "PRIVATE_TEST_INPUT": "not published"}

            def run(args, **kwargs):
                self.assertEqual(kwargs, {"env": environment})
                if args[:2] == ["cargo", "tree"]:
                    return "hive v0.1.0\nwidget v1.0.0\n"
                if args[:2] == ["cargo", "metadata"]:
                    return json.dumps({"packages": entries})
                if args == ["rustc", "--print", "sysroot"]:
                    return str(root / "toolchain")
                self.assertEqual(args, ["rustc", "-V"])
                return "rustc fixture"

            with patch.object(licenses, "ROOT", root), patch.object(licenses, "run", run):
                tracked = [str(p.relative_to(root)) for p in root.rglob("*") if p.is_file()]
                source = Snapshot(root, Git(root, tracked))
                graph = licenses.dependency_metadata(
                    "hive", [], "fixture", env=environment, source=source
                )
                records, text = licenses.collect(graph, env=environment, source=source)
                self.assertEqual([r["name"] for r in records], ["widget", "rust-standard-library"])
                self.assertIn("Third-party widget permission notice.", text)
                self.assertIn("path dependency from the build checkout", text)
                self.assertNotIn("crates.io/api/v1/crates/widget", text)
                self.assertFalse(
                    licenses.first_party(dict(entries[0], manifest_path=str(vendor / "Cargo.toml")))
                )
                (vendor / "LICENSE").unlink()
                with self.assertRaisesRegex(ValueError, "Missing license text: widget"):
                    licenses.collect(graph, env=environment, source=source)

    def test_build_only_external_path_dependency_is_rejected_before_selection(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve() / "project"
            root.mkdir()
            (root / "Cargo.toml").write_text("workspace")
            source = Snapshot(root, Git(root, ["Cargo.toml"]))
            entry = dict(
                name="generator",
                version="1.0.0",
                source=None,
                manifest_path=str(root.parent / "generator/Cargo.toml"),
                targets=[],
            )

            def run(args, **kwargs):
                if args[:2] == ["cargo", "tree"]:
                    return ""
                return json.dumps({"packages": [entry]})

            with patch.object(licenses, "run", run):
                with self.assertRaisesRegex(ValueError, "inside the source checkout"):
                    licenses.dependency_metadata("hive", [], "fixture", env={}, source=source)

    def test_unattributed_in_checkout_package_is_rejected(self):
        for location in ("target/generator", "vendor/generator"):
            with self.subTest(location=location), tempfile.TemporaryDirectory() as directory:
                root = Path(directory).resolve()
                (root / "Cargo.toml").write_text("workspace")
                package = root / location
                package.mkdir(parents=True)
                (package / "Cargo.toml").write_text("build-only generator")
                source = Snapshot(root, Git(root, ["Cargo.toml"]))
                entry = dict(
                    name="generator",
                    version="1.0.0",
                    source=None,
                    manifest_path=str(package / "Cargo.toml"),
                    targets=[],
                )
                with self.assertRaisesRegex(ValueError, "not bound to the source snapshot"):
                    licenses.validate_paths({"packages": [entry]}, source)

    def test_vendor_nested_target_inputs_refuse_compile_and_notice_admission(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            package = root / "vendor/widget"
            (package / "src").mkdir(parents=True)
            (package / "target").mkdir()
            (package / "Cargo.toml").write_text("vendored fixture")
            (package / "src/lib.rs").write_text('include!("../target/input.rs");')
            (package / "target/input.rs").write_text("pub fn generated() {}")
            (package / "target/LICENSE").write_text("unattributed notice")
            source = Snapshot(
                root, Git(root, ["vendor/widget/Cargo.toml", "vendor/widget/src/lib.rs"])
            )
            entry = dict(
                name="widget",
                version="1.0.0",
                source=None,
                manifest_path=str(package / "Cargo.toml"),
                targets=[{"src_path": str(package / "src/lib.rs")}],
            )
            metadata = {"packages": [entry]}

            def run(args, **kwargs):
                return "widget v1.0.0\n" if args[:2] == ["cargo", "tree"] else json.dumps(metadata)

            with (
                patch.object(licenses, "run", run),
                patch.object(licenses, "package_notice") as notice,
            ):
                with self.assertRaisesRegex(ValueError, "not bound to the source snapshot"):
                    licenses.dependency_metadata("hive", [], "fixture", env={}, source=source)
                with self.assertRaisesRegex(ValueError, "not bound to the source snapshot"):
                    licenses.collect(({("widget", "1.0.0")}, metadata), env={}, source=source)
                notice.assert_not_called()

    def test_declared_shared_license_must_be_confined_and_source_bound(self):
        for kind in ("external", "unbound", "bound"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as directory:
                root = Path(directory).resolve() / "project"
                package = root / "vendor/widget"
                package.mkdir(parents=True)
                (package / "Cargo.toml").write_text("vendored fixture")
                name = "../../../LICENSE" if kind == "external" else "../shared/LICENSE"
                license_file = package / name
                license_file.parent.mkdir(parents=True, exist_ok=True)
                license_file.write_text("shared permission notice")
                tracked = ["vendor/widget/Cargo.toml"]
                if kind == "bound":
                    tracked.append("vendor/shared/LICENSE")
                source = Snapshot(root, Git(root, tracked))
                entry = dict(
                    name="widget",
                    version="1.0.0",
                    source=None,
                    license="MIT",
                    manifest_path=str(package / "Cargo.toml"),
                    targets=[],
                    license_file=name,
                )
                metadata = {"packages": [entry]}

                def run(args, **kwargs):
                    return (
                        "widget v1.0.0\n" if args[:2] == ["cargo", "tree"] else json.dumps(metadata)
                    )

                with patch.object(licenses, "run", run):
                    if kind == "bound":
                        licenses.dependency_metadata("hive", [], "fixture", env={}, source=source)
                        self.assertIn(
                            "shared permission notice", "".join(licenses.package_notice(entry)[1])
                        )
                    else:
                        with self.assertRaises(ValueError):
                            licenses.dependency_metadata(
                                "hive", [], "fixture", env={}, source=source
                            )
                        with self.assertRaises(ValueError):
                            licenses.collect(
                                ({("widget", "1.0.0")}, metadata), env={}, source=source
                            )


if __name__ == "__main__":
    unittest.main()
