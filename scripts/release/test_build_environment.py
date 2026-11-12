"""Cargo configuration and environment cannot silently change a declared plain build."""

import json
from pathlib import Path
import tempfile
import unittest
from .build_environment import Environment, compiler_identity
from .cargo_config import validate


class BuildEnvironmentTests(unittest.TestCase):
    def test_optional_llama_floor_is_explicit_and_does_not_change_cargo_defaults(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            inherited = {"HOME": str(root), "PATH": "/fixture/bin"}
            ordinary = Environment(root, inherited, system="Darwin")
            llama = Environment(root, inherited, system="Darwin", macos_deployment_target="13.3")
            self.assertEqual(ordinary.values["MACOSX_DEPLOYMENT_TARGET"], "11.0")
            self.assertEqual(llama.values["MACOSX_DEPLOYMENT_TARGET"], "13.3")
            self.assertEqual(llama.public()["macos_deployment_target"], "13.3")
            with self.assertRaises(ValueError):
                Environment(root, inherited, system="Darwin", macos_deployment_target="14.0")
            with self.assertRaises(ValueError):
                Environment(root, inherited | {"MACOSX_DEPLOYMENT_TARGET": "13.3"}, system="Darwin")

    def test_private_build_paths_compile_to_public_names(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            inherited = {"HOME": str(root / "home"), "PATH": "/fixture/bin"}
            admitted = Environment(root, inherited, system="Darwin")
            self.assertNotIn("CARGO_ENCODED_RUSTFLAGS", admitted.for_target(root / "target"))
            self.assertNotIn("remapped_path_prefixes", admitted.public())
            commit = "a" * 40
            compiler = f"rustc 1.0.0 (aaaaaaa 2026-01-01)\ncommit-hash: {commit}\n"
            env = admitted.for_target(root / "target")
            admitted.remap_paths(env, str(root / "sysroot"), compiler)
            self.assertEqual(
                env["CARGO_ENCODED_RUSTFLAGS"].split("\x1f"),
                [
                    f"--remap-path-prefix={root}=/hive",
                    f"--remap-path-prefix={root}/home/.cargo=/cargo",
                    f"--remap-path-prefix={root}/sysroot/lib/rustlib/src/rust=/rustc/{commit}",
                    f"--remap-path-prefix={root}/target=/target",
                ],
            )
            public = admitted.public()
            self.assertEqual(
                public["remapped_path_prefixes"], ["/hive", "/cargo", f"/rustc/{commit}", "/target"]
            )
            self.assertNotIn(str(root), json.dumps(public))

    def test_harmless_config_ci_values_and_transport_remain_private(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            config = root / ".cargo/config.toml"
            config.parent.mkdir()
            config.write_text(
                "[build]\njobs=32\nincremental=true\n[net]\ngit-fetch-with-cli=true\n"
                '[http]\nproxy="http://private-password@127.0.0.1/#literal" # comment\n'
                '[registries.crates-io]\nprotocol="sparse"\n'
            )
            inherited = {
                "HOME": str(root),
                "PATH": "/fixture/bin",
                "CARGO_BUILD_JOBS": "24",
                "CARGO_INCREMENTAL": "1",
                "CARGO_TERM_COLOR": "always",
                "HTTPS_PROXY": "private-transport",
                "OTHER_SECRET": "not forwarded",
            }
            admitted = Environment(root, inherited, system="Darwin")
            env = admitted.for_target(root / "private-target")
            self.assertEqual(env["CARGO_BUILD_JOBS"], "4")
            self.assertEqual(env["CARGO_INCREMENTAL"], "0")
            self.assertEqual(env["CARGO_TERM_COLOR"], "never")
            self.assertEqual(env["HTTPS_PROXY"], "private-transport")
            self.assertNotIn("OTHER_SECRET", env)
            public = json.dumps(admitted.public())
            for private in (str(root), "private-password", "private-transport", "CARGO_TARGET_DIR"):
                self.assertNotIn(private, public)
            admitted.config.check()
            config.write_text("[net]\noffline=true\n")
            with self.assertRaisesRegex(ValueError, "changed during"):
                admitted.config.check()

    def test_build_altering_configuration_and_unparsed_toml_refuse(self):
        for text in (
            '[env]\nCFLAGS="-march=native"',
            '[target.native]\nlinker="other"',
            '[target.native.sqlite3]\nrustc-link-lib="other"',
            'paths=["elsewhere"]',
            '[source.crates-io]\nreplace-with="mirror"',
            '[patch.crates-io]\nfoo="elsewhere"',
            "[profile.release]\nopt-level=0",
            '[build]\nrustflags="-Copt-level=0"',
            "[net]\noffline={value=true}",
            "[net]\noffline=true\noffline=false",
            "[net]\noffline=true\n[net]\nretry=2",
            '[net]\n"offline"=true',
            "[net]\noffline=true\nhttp.timeout=3",
            '[http]\nproxy="""multiline"""',
        ):
            with self.subTest(text=text), self.assertRaises(ValueError):
                validate(text.encode())

    def test_native_build_flags_refuse_before_a_command_can_start(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for key in (
                "RUSTFLAGS",
                "CARGO_PROFILE_RELEASE_OPT_LEVEL",
                "CARGO_TARGET_DIR",
                "CARGO_BUILD_TARGET",
                "CC_aarch64_apple_darwin",
                "aarch64_apple_darwin_CC",
                "TARGET_CFLAGS",
                "CFLAGS",
                "LIBSQLITE3_FLAGS",
                "CMAKE_TOOLCHAIN_FILE",
                "SDKROOT",
                "AWS_LC_SYS_CFLAGS",
                "OPENSSL_DIR",
            ):
                with self.subTest(key=key), self.assertRaises(ValueError):
                    Environment(root, {"HOME": str(root), key: "private-override"})

    def test_compiler_identity_does_not_publish_arbitrary_output_or_paths(self):
        text = (
            "rustc 1.98.1 (abcdef123 2026-09-01)\nbinary: rustc\ncommit-hash: "
            + "a" * 40
            + "\ncommit-date: 2026-09-01\nhost: aarch64-apple-darwin\n"
            "release: 1.98.1\nLLVM version: 22.1.2\n"
        )
        self.assertEqual(compiler_identity(text, "aarch64-apple-darwin"), text.strip())
        for invalid in (
            text + "private-path: /private/credential\n",
            text.replace("binary: rustc", "binary: /private/rustc"),
        ):
            with self.assertRaises(ValueError):
                compiler_identity(invalid, "aarch64-apple-darwin")
