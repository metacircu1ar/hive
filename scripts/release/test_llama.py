"""The shared llama-server pin, its exact CMake options, record and notices."""

import json
from pathlib import Path
import tempfile
import unittest

from . import llama
from .licenses import source_notices
from .test_package_fixture import FAKE_PIN


class Pin(unittest.TestCase):
    def test_complete_pin_loads_and_every_placeholder_or_shape_change_is_refused(self):
        self.assertEqual(llama.load(json.dumps(FAKE_PIN).encode()), FAKE_PIN)
        for path, value in (
            (("llama_cpp", "tree"), "UNVERIFIED: record the tree"),
            (("llama_cpp", "commit"), "C" * 40),
            (("llama_cpp", "repository"), "https://example.invalid/llama.cpp.git"),
            (("model", "sha256"), "f" * 63),
            (("model", "bytes"), 0),
            (("model", "bytes"), True),
            (("model", "file"), "../model.gguf"),
            (("model", "license"), "MIT"),
        ):
            with self.subTest(path=path, value=value):
                broken = json.loads(json.dumps(FAKE_PIN))
                broken[path[0]][path[1]] = value
                with self.assertRaises(ValueError):
                    llama.load(json.dumps(broken).encode())
        extra = json.loads(json.dumps(FAKE_PIN))
        extra["model"]["mirror"] = "https://example.invalid"
        with self.assertRaises(ValueError):
            llama.load(json.dumps(extra).encode())
        with self.assertRaises(ValueError):
            llama.load(b'{"llama_cpp": {}, "llama_cpp": {}, "model": {}}')

    def test_shipped_pin_is_the_only_copy_and_has_no_placeholder(self):
        # Fails until the verified llama.cpp tree ID replaces its placeholder.
        pin = llama.load()
        self.assertEqual(pin["model"]["file"], "Qwen3-Embedding-8B-Q8_0.gguf")
        dependencies = json.loads((llama.ROOT / "e2e/dependencies.json").read_text())
        self.assertEqual(set(dependencies), {"ubuntu"})

    def test_model_url_is_the_pinned_revision(self):
        self.assertEqual(
            llama.model_url(FAKE_PIN),
            "https://huggingface.co/Example/model/resolve/" + "e" * 40 + "/model.gguf",
        )


class Options(unittest.TestCase):
    def test_every_target_disables_downloads_caches_and_dynamic_backends(self):
        for target in (
            "aarch64-apple-darwin",
            "aarch64-unknown-linux-gnu",
            "x86_64-unknown-linux-gnu",
        ):
            with self.subTest(target=target):
                values = llama.options(target)
                for name in (
                    "BUILD_SHARED_LIBS",
                    "GGML_NATIVE",
                    "GGML_BACKEND_DL",
                    "GGML_CCACHE",
                    "GGML_OPENMP",
                    "LLAMA_OPENSSL",
                    "LLAMA_BUILD_UI",
                    "LLAMA_USE_PREBUILT_UI",
                    "LLAMA_BUILD_TESTS",
                    "LLAMA_BUILD_EXAMPLES",
                ):
                    self.assertEqual(values[name], "OFF", name)
                self.assertEqual(values["CMAKE_BUILD_TYPE"], "Release")
                self.assertNotIn("LLAMA_CURL", values)
                self.assertEqual(values, llama.options(target))

    def test_platform_specific_backends_and_baselines(self):
        mac = llama.options("aarch64-apple-darwin")
        self.assertEqual(mac["GGML_METAL"], "ON")
        self.assertEqual(mac["GGML_METAL_EMBED_LIBRARY"], "ON")
        self.assertEqual(mac["GGML_ACCELERATE"], "ON")
        self.assertEqual(mac["CMAKE_OSX_DEPLOYMENT_TARGET"], "13.3")
        self.assertEqual(mac["CMAKE_EXE_LINKER_FLAGS"], "")
        arm = llama.options("aarch64-unknown-linux-gnu")
        self.assertEqual(arm["GGML_METAL"], "OFF")
        self.assertEqual(arm["GGML_CPU_ARM_ARCH"], llama.LINUX_ARM64_ARCH)
        self.assertEqual(arm["CMAKE_EXE_LINKER_FLAGS"], "-static-libstdc++ -static-libgcc")
        x86 = llama.options("x86_64-unknown-linux-gnu")
        for name in ("GGML_AVX2", "GGML_FMA", "GGML_F16C", "GGML_BMI2"):
            self.assertEqual(x86[name], "ON")
        self.assertEqual(x86["GGML_AVX512"], "OFF")
        self.assertNotIn("GGML_CPU_ARM_ARCH", x86)
        for target in ("riscv64-unknown-linux-gnu", "aarch64-pc-windows-msvc"):
            with self.assertRaises(ValueError):
                llama.options(target)


class Record(unittest.TestCase):
    def record(self, target="aarch64-unknown-linux-gnu"):
        return {
            "repository": FAKE_PIN["llama_cpp"]["repository"],
            "commit": FAKE_PIN["llama_cpp"]["commit"],
            "tree": FAKE_PIN["llama_cpp"]["tree"],
            "cmake_options": llama.options(target),
            "cmake": "cmake version 3.28.3",
            "compiler": {"c": "cc (Ubuntu 11.4.0) 11.4.0", "cxx": "c++ (Ubuntu 11.4.0) 11.4.0"},
        }

    def test_record_binds_pin_options_and_bounded_toolchain_identity(self):
        llama.check_record(self.record(), "aarch64-unknown-linux-gnu", pin=FAKE_PIN)
        for field, value in (
            ("tree", "0" * 40),
            ("cmake", "x" * 201),
            ("compiler", {"c": "cc"}),
            ("cmake_options", llama.options("x86_64-unknown-linux-gnu")),
        ):
            with self.subTest(field=field):
                record = self.record()
                record[field] = value
                with self.assertRaises(ValueError):
                    llama.check_record(record, "aarch64-unknown-linux-gnu", pin=FAKE_PIN)

    def test_version_line_must_name_the_pinned_commit(self):
        commit = FAKE_PIN["llama_cpp"]["commit"]
        llama.check_version(f"version: 0.6.0-dev (build 1, commit {commit[:7]})", commit)
        llama.check_version(f"version: 0.6.0-dev (build 7012, commit {commit})", commit)
        for line in (
            "version: 0.6.0-dev (build 1, commit 0000000)",
            f"version: 0.6.0-dev (build 1, commit {commit[:6]})",
            f"version 1 ({commit[:7]})",
            None,
        ):
            with self.subTest(line=line), self.assertRaises(ValueError):
                llama.check_version(line, commit)


class Notices(unittest.TestCase):
    def tree(self, root, vendored=("stb",)):
        (root / "vendor").mkdir()
        for name in vendored:
            (root / "vendor" / name).mkdir()
        (root / "LICENSE").write_text("MIT License\nllama.cpp authors\n")
        (root / "vendor/stb/stb_image.h").write_text(
            "code\n" * 10 + "This software is available under 2 licenses\nALTERNATIVE A\n"
        )

    def declarations(self):
        return (
            ("llama.cpp and ggml", "MIT", (("LICENSE", None),)),
            (
                "stb_image",
                "MIT OR Unlicense",
                (("vendor/stb/stb_image.h", "This software is available under 2 licenses"),),
            ),
        )

    def test_whole_files_and_bounded_excerpts_are_collected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.tree(root)
            records, text = source_notices(
                root,
                self.declarations(),
                vendor={"stb": "stb_image"},
                not_compiled=frozenset(),
                version="c" * 40,
                origin=llama.REPOSITORY,
            )
        self.assertEqual([r["name"] for r in records], ["llama.cpp and ggml", "stb_image"])
        self.assertIn("llama.cpp authors", text)
        self.assertIn("ALTERNATIVE A", text)
        self.assertNotIn("code\ncode", text)

    def test_unreviewed_vendor_missing_text_and_license_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.tree(root, vendored=("stb", "unknown"))
            arguments = dict(version="c" * 40, origin=llama.REPOSITORY)
            with self.assertRaisesRegex(ValueError, "vendor/unknown"):
                source_notices(
                    root,
                    self.declarations(),
                    vendor={"stb": "stb_image"},
                    not_compiled=frozenset(),
                    **arguments,
                )
            records, _ = source_notices(
                root,
                self.declarations(),
                vendor={"stb": "stb_image"},
                not_compiled=frozenset({"unknown"}),
                **arguments,
            )
            self.assertEqual(len(records), 2)
            (root / "LICENSE").unlink()
            with self.assertRaisesRegex(ValueError, "Missing license text"):
                source_notices(
                    root,
                    self.declarations(),
                    vendor={"stb": "stb_image"},
                    not_compiled=frozenset({"unknown"}),
                    **arguments,
                )
            with self.assertRaisesRegex(ValueError, "Unreviewed native component license"):
                source_notices(
                    root,
                    (("component", "GPL-3.0", ()),),
                    vendor={},
                    not_compiled=frozenset({"stb", "unknown"}),
                    **arguments,
                )


if __name__ == "__main__":
    unittest.main()
