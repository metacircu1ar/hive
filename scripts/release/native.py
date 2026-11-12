"""Read actual macOS deployment requirements from bounded Mach-O load commands."""

import struct

MAX_COMMAND_BYTES = 1024 * 1024
MACOS_FLOOR = 11 << 16
LLAMA_MACOS_DEPLOYMENT_TARGET = "13.3"
LLAMA_MACOS_FLOOR = (13 << 16) | (3 << 8)


def macos_minimum(stream, header, *, binary_name=None):
    floor = LLAMA_MACOS_FLOOR if binary_name == "comb-llama-server" else MACOS_FLOOR
    supported = LLAMA_MACOS_DEPLOYMENT_TARGET if binary_name == "comb-llama-server" else "11.0"
    count, size = struct.unpack_from("<II", header, 16)
    if size > MAX_COMMAND_BYTES or count > size // 8:
        raise ValueError("Invalid Mach-O load-command bounds")
    commands = stream.read(size)
    if len(commands) != size:
        raise ValueError("Truncated Mach-O load commands")
    minimum, offset = None, 0
    for _ in range(count):
        if offset + 8 > size:
            raise ValueError("Truncated Mach-O load command")
        kind, length = struct.unpack_from("<II", commands, offset)
        if length < 8 or length % 8 or offset + length > size:
            raise ValueError("Invalid Mach-O load-command size")
        version = None
        if kind == 0x32:  # LC_BUILD_VERSION
            if length < 24:
                raise ValueError("Truncated Mach-O build version")
            platform, version, _, tools = struct.unpack_from("<IIII", commands, offset + 8)
            if platform != 1 or length != 24 + tools * 8:
                raise ValueError("Require a native macOS build-version command")
        elif kind == 0x24:  # LC_VERSION_MIN_MACOSX
            if length != 16:
                raise ValueError("Invalid Mach-O minimum-version command")
            version = struct.unpack_from("<I", commands, offset + 8)[0]
        elif kind in (0x25, 0x2F, 0x30):  # iOS, tvOS, watchOS
            raise ValueError("Require a native macOS deployment target")
        if version is not None:
            if not version or version > floor:
                raise ValueError(
                    f"Release requires macOS newer than the supported {supported} floor"
                )
            minimum = version if minimum is None else max(minimum, version)
        offset += length
    if offset != size or minimum is None:
        raise ValueError("Missing or invalid macOS deployment requirement")
    return f"{minimum >> 16}.{(minimum >> 8) & 255}.{minimum & 255}"
