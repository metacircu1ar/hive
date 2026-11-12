"""One admitted environment for Cargo, dependency observation and toolchain probes."""

import os
import platform
import re
from pathlib import Path
from .cargo_config import Snapshot
from .native import LLAMA_MACOS_DEPLOYMENT_TARGET

FORWARD = {
    "PATH",
    "HOME",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "TMPDIR",
    "TMP",
    "TEMP",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
}
FIXED = {
    "CARGO_BUILD_JOBS": "4",
    "CARGO_INCREMENTAL": "0",
    "CARGO_TERM_COLOR": "never",
    "LC_ALL": "C",
    "TZ": "UTC",
}
DENIED = re.compile(
    r"(?:RUSTFLAGS|CARGO_ENCODED_RUSTFLAGS|RUSTC|RUSTDOC|RUSTC_WRAPPER|RUSTC_WORKSPACE_WRAPPER"
    r"|CARGO_TARGET_DIR|CARGO_PROFILE_.*|CARGO_TARGET_.*|CARGO_BUILD_.*"
    r"|(?:.*_)?(?:CC|CXX|AR|CFLAGS|CXXFLAGS|CPPFLAGS|LDFLAGS)(?:_.*)?"
    r"|SDKROOT|DEVELOPER_DIR|CMAKE.*|AWS_LC_SYS_.*|LIBSQLITE3_.*|OPENSSL_.*|RING_.*|BINDGEN_.*)"
)


def compiler_identity(output, target):
    lines = output.splitlines()
    if not lines or not re.fullmatch(
        r"rustc [0-9][A-Za-z0-9.+-]{0,63} \([0-9a-f]{7,40} [0-9]{4}-[0-9]{2}-[0-9]{2}\)",
        lines[0],
    ):
        raise ValueError("Compiler must report a bounded official version identity")
    formats = {
        "binary": r"rustc",
        "commit-hash": r"[0-9a-f]{40}|unknown",
        "commit-date": r"[0-9]{4}-[0-9]{2}-[0-9]{2}|unknown",
        "host": re.escape(target),
        "release": r"[0-9][A-Za-z0-9.+-]{0,63}",
        "LLVM version": r"[0-9.]{1,32}",
    }
    seen = set()
    for line in lines[1:]:
        key, separator, value = line.partition(": ")
        if (
            not separator
            or key not in formats
            or key in seen
            or not re.fullmatch(formats[key], value)
        ):
            raise ValueError("Compiler identity contains an unsupported field")
        seen.add(key)
    if seen != set(formats):
        raise ValueError("Compiler identity is incomplete")
    return "\n".join(lines)


class Environment:
    def __init__(self, root, inherited=None, *, system=None, macos_deployment_target="11.0"):
        inherited = dict(os.environ if inherited is None else inherited)
        self.root = Path(root).resolve()
        self.system = platform.system() if system is None else system
        if macos_deployment_target not in ("11.0", LLAMA_MACOS_DEPLOYMENT_TARGET):
            raise ValueError("Unsupported release deployment target")
        self.macos_deployment_target = macos_deployment_target
        for key, value in inherited.items():
            if value and key not in FIXED and DENIED.fullmatch(key):
                raise ValueError("Build-altering environment override is not admitted")
        deployment = inherited.get("MACOSX_DEPLOYMENT_TARGET")
        if deployment and deployment not in ("11.0", macos_deployment_target):
            raise ValueError(f"Release deployment target must be macOS {macos_deployment_target}")
        self.values = {key: value for key, value in inherited.items() if key in FORWARD}
        self.values.update(FIXED)
        if self.system == "Darwin":
            self.values["MACOSX_DEPLOYMENT_TARGET"] = macos_deployment_target
        user_home = Path(self.values.get("HOME") or Path.home())
        cargo_home = Path(self.values.get("CARGO_HOME") or user_home / ".cargo")
        if not cargo_home.is_absolute():
            cargo_home = self.root / cargo_home
        self.cargo_home = cargo_home.resolve()
        self.config = Snapshot(self.root, cargo_home)
        self.remapped = None

    def for_target(self, target):
        return self.values | {"CARGO_TARGET_DIR": str(Path(target).resolve())}

    def remap_paths(self, env, sysroot, compiler):
        """Compile private build paths in this Cargo environment to fixed public names."""
        commit = re.search(r"^commit-hash: (.+)$", compiler, re.M).group(1)
        # rustc applies the last matching prefix, so nested paths follow the source root.
        remaps = (
            (self.root, "/hive"),
            (self.cargo_home, "/cargo"),
            (Path(sysroot).resolve() / "lib/rustlib/src/rust", "/rustc/" + commit),
            (Path(env["CARGO_TARGET_DIR"]), "/target"),
        )
        self.remapped = [public for _, public in remaps]
        env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(
            f"--remap-path-prefix={private}={public}" for private, public in remaps
        )

    def public(self):
        value = {"build_jobs": 4, "incremental": False, "term_color": "never"}
        if self.system == "Darwin":
            value["macos_deployment_target"] = self.macos_deployment_target
        if self.remapped is not None:
            value["remapped_path_prefixes"] = self.remapped
        return value
