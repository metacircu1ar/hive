"""Actual macOS deployment metadata, independent of a build environment setting."""

import io
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch
from .common import inspect_binary
from .native import MAX_COMMAND_BYTES, macos_minimum


def header(count, size):
    data = bytearray(32)
    struct.pack_into("<II", data, 16, count, size)
    return data


class NativeFloorTests(unittest.TestCase):
    def test_only_the_optional_llama_binary_may_require_macos_13_3(self):
        command = struct.pack("<IIIIII", 0x32, 24, 1, (13 << 16) | (3 << 8), 26 << 16, 0)
        for name in [None, "hivectl", "comb", "hive-install"]:
            with self.subTest(name=name), self.assertRaises(ValueError):
                macos_minimum(io.BytesIO(command), header(1, len(command)), binary_name=name)
        self.assertEqual(
            macos_minimum(
                io.BytesIO(command), header(1, len(command)), binary_name="comb-llama-server"
            ),
            "13.3.0",
        )
        newer = bytearray(command)
        struct.pack_into("<I", newer, 12, 14 << 16)
        with self.assertRaisesRegex(ValueError, "supported 13.3 floor"):
            macos_minimum(io.BytesIO(newer), header(1, len(newer)), binary_name="comb-llama-server")

    def test_both_version_forms_check_minimum_and_ignore_sdk(self):
        for command in (
            struct.pack("<IIIIII", 0x32, 24, 1, 11 << 16, 26 << 16, 0),
            struct.pack("<IIII", 0x24, 16, 11 << 16, 26 << 16),
        ):
            self.assertEqual(macos_minimum(io.BytesIO(command), header(1, len(command))), "11.0.0")
            invalid = bytearray(command)
            struct.pack_into("<I", invalid, 12 if len(command) == 24 else 8, 12 << 16)
            with self.assertRaisesRegex(ValueError, "supported 11.0 floor"):
                macos_minimum(io.BytesIO(invalid), header(1, len(invalid)))

    def test_missing_truncated_wrong_platform_and_invalid_command_sizes_refuse(self):
        for commands, count, size in (
            (b"", 0, 0),
            (b"", 1, MAX_COMMAND_BYTES + 1),
            (b"", 1, 24),
            (struct.pack("<IIIIII", 0x32, 24, 2, 11 << 16, 0, 0), 1, 24),
            (struct.pack("<IIIIII", 0x32, 24, 1, 11 << 16, 0, 1), 1, 24),
            (struct.pack("<II", 0x32, 0), 1, 8),
            (struct.pack("<IIII", 0x24, 8, 11 << 16, 0), 1, 16),
            (struct.pack("<IIII", 0x25, 16, 11 << 16, 0), 1, 16),
        ):
            with (
                self.subTest(commands=commands, count=count, size=size),
                self.assertRaises(ValueError),
            ):
                macos_minimum(io.BytesIO(commands), header(count, size))

    def test_later_command_cannot_hide_a_higher_minimum(self):
        commands = struct.pack("<IIII", 0x24, 16, 11 << 16, 0) + struct.pack(
            "<IIIIII", 0x32, 24, 1, 12 << 16, 0, 0
        )
        with self.assertRaises(ValueError):
            macos_minimum(io.BytesIO(commands), header(2, len(commands)))


class BinaryInspectionTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def test_native_audit_rejects_wrong_architecture_and_external_libraries(self):
        binary = self.root / "native"
        native = bytearray(b"\xcf\xfa\xed\xfe" + struct.pack("<I", 0x100000C) + bytes(24))
        struct.pack_into("<II", native, 16, 1, 24)
        binary.write_bytes(native + struct.pack("<IIIIII", 0x32, 24, 1, 11 << 16, 26 << 16, 0))
        with self.assertRaisesRegex(ValueError, "architecture"):
            inspect_binary(binary, "x86_64-apple-darwin")
        with (
            patch(
                "release.common.run",
                return_value="native:\n /tmp/build/libcustom.dylib (compatibility version 1.0.0)",
            ),
            self.assertRaisesRegex(ValueError, "Non-system shared library"),
        ):
            inspect_binary(binary, "aarch64-apple-darwin")

    def test_linux_records_glibc_and_refuses_the_cpp_runtime(self):
        binary = self.root / "elf"
        native = bytearray(32)
        native[:6] = b"\x7fELF\x02\x01"
        struct.pack_into("<H", native, 18, 62)
        binary.write_bytes(native)
        libc = "libc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x123)"
        with patch("release.common.run", side_effect=[libc, "GLIBC_2.17 GLIBC_2.35"]):
            record = inspect_binary(binary, "x86_64-unknown-linux-gnu")
        self.assertEqual(
            record, {"libraries": ["/lib/x86_64-linux-gnu/libc.so.6"], "required_glibc": "2.35"}
        )
        cpp = "libstdc++.so.6 => /lib/x86_64-linux-gnu/libstdc++.so.6 (0x123)"
        for listing, symbols in (
            (cpp, "GLIBC_2.35"),
            ("libc++.so.1 => /usr/lib/libc++.so.1 (0x123)", "GLIBC_2.35"),
            ("libc++abi.so.1 => /usr/lib/libc++abi.so.1 (0x123)", "GLIBC_2.35"),
            (libc, "GLIBC_2.35 GLIBCXX_3.4.29"),
            (libc, "GLIBC_2.35 CXXABI_1.3.13"),
        ):
            with (
                self.subTest(listing=listing, symbols=symbols),
                patch("release.common.run", side_effect=[listing, symbols]),
                self.assertRaisesRegex(ValueError, "C\\+\\+ runtime"),
            ):
                inspect_binary(binary, "x86_64-unknown-linux-gnu")
