"""The pinned static llama-server: one shared pin, exact CMake options and its record.

The pin in comb/src/llama/pin.json is the only copy of the llama.cpp commit and
tree and of the Qwen revision, SHA-256 and size. The release builder, Comb and the
opt-in E2E suite all read it; a placeholder or malformed value refuses each of them.
"""

import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil

from .common import ROOT
from .licenses import source_notices
from .process import run as run_process
from .native import LLAMA_MACOS_DEPLOYMENT_TARGET
from . import llama_notices
from .llama_notices import NOTICES, VENDOR, NOT_COMPILED

PIN = "comb/src/llama/pin.json"
PART = "comb-llama"
BINARY = "comb-llama-server"
TARGET = "llama-server"
REPOSITORY = "https://github.com/ggml-org/llama.cpp.git"
JOBS = 4
# The private build tree is never recorded; BUILD stands for it.
COMMAND = ["cmake", "--build", "BUILD", "--config", "Release", "--target", TARGET]
# GGML_NATIVE=OFF baseline for Linux arm64. The reviewed alternative is
# "armv8.2-a+dotprod": several times faster int8 CPU paths (Graviton2+, Ampere,
# Apple-silicon VMs), but it refuses older arm64 cores. Decide from measurements.
LINUX_ARM64_ARCH = "armv8-a"
# Metal/Accelerate at this pin call macOS 12.0/13.3 APIs without guards.
MACOS_DEPLOYMENT_TARGET = LLAMA_MACOS_DEPLOYMENT_TARGET
FETCH_SECONDS = 1800
CONFIGURE_SECONDS = 600
BUILD_SECONDS = 3600
IDENTITY_BYTES = 200
HEX40 = re.compile(r"[0-9a-f]{40}")
HEX64 = re.compile(r"[0-9a-f]{64}")
VERSION = re.compile(
    r"version: [0-9]{1,9}\.[0-9]{1,9}\.[0-9]{1,9}(?:-[A-Za-z0-9.-]{1,64})? "
    r"\(build [0-9]{1,9}, commit ([0-9a-f]{7,40})\)"
)
MODEL_FIELDS = {"repository", "revision", "file", "sha256", "bytes", "license"}

# Every option is explicit, so the record and E2E receipts name the whole build.
COMMON = {
    "CMAKE_BUILD_TYPE": "Release",
    "BUILD_SHARED_LIBS": "OFF",
    "CMAKE_C_FLAGS": "",
    "CMAKE_CXX_FLAGS": "",
    "GGML_NATIVE": "OFF",
    "GGML_BACKEND_DL": "OFF",
    "GGML_CPU_ALL_VARIANTS": "OFF",
    "GGML_CCACHE": "OFF",
    "GGML_OPENMP": "OFF",
    "GGML_OPENMP_FETCH": "OFF",
    "GGML_CPU_KLEIDIAI": "OFF",
    "GGML_LLAMAFILE": "ON",
    "GGML_CUDA": "OFF",
    "GGML_HIP": "OFF",
    "GGML_MUSA": "OFF",
    "GGML_VULKAN": "OFF",
    "GGML_SYCL": "OFF",
    "GGML_RPC": "OFF",
    "GGML_CANN": "OFF",
    "GGML_OPENCL": "OFF",
    "GGML_WEBGPU": "OFF",
    "GGML_ZDNN": "OFF",
    "LLAMA_OPENSSL": "OFF",
    "LLAMA_LLGUIDANCE": "OFF",
    "LLAMA_SUBPROCESS": "OFF",
    "MTMD_VIDEO": "OFF",
    # LLAMA_USE_PREBUILT_UI defaults to on and would download the web UI.
    "LLAMA_BUILD_UI": "OFF",
    "LLAMA_USE_PREBUILT_UI": "OFF",
    "LLAMA_BUILD_BORINGSSL": "OFF",
    "LLAMA_BUILD_LIBRESSL": "OFF",
    "LLAMA_BUILD_TESTS": "OFF",
    "LLAMA_BUILD_EXAMPLES": "OFF",
    "LLAMA_BUILD_APP": "OFF",
    "LLAMA_BUILD_COMMON": "ON",
    "LLAMA_BUILD_TOOLS": "ON",
    "LLAMA_BUILD_SERVER": "ON",
}
# x86-64 keeps ggml's portable defaults: a Haswell-class CPU is the stated floor.
HASWELL = {
    "GGML_SSE42": "ON",
    "GGML_AVX": "ON",
    "GGML_AVX2": "ON",
    "GGML_BMI2": "ON",
    "GGML_FMA": "ON",
    "GGML_F16C": "ON",
    "GGML_AVX_VNNI": "OFF",
    "GGML_AVX512": "OFF",
    "GGML_AVX512_VBMI": "OFF",
    "GGML_AVX512_VNNI": "OFF",
    "GGML_AVX512_BF16": "OFF",
    "GGML_AMX_TILE": "OFF",
    "GGML_AMX_INT8": "OFF",
    "GGML_AMX_BF16": "OFF",
}


def _unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Duplicate llama-server pin key")
        result[key] = value
    return result


def load(data=None):
    """Read the one shared pin; any placeholder refuses the caller."""
    raw = (ROOT / PIN).read_bytes() if data is None else data
    value = json.loads(raw, object_pairs_hook=_unique)
    validate(value)
    return value


def validate(value):
    if not isinstance(value, dict) or set(value) != {"llama_cpp", "model"}:
        raise ValueError("Invalid llama-server pin shape")
    source = value["llama_cpp"]
    if (
        not isinstance(source, dict)
        or set(source) != {"repository", "commit", "tree"}
        or source["repository"] != REPOSITORY
    ):
        raise ValueError("Invalid llama.cpp source pin")
    for field in ("commit", "tree"):
        if not isinstance(source[field], str) or not HEX40.fullmatch(source[field]):
            raise ValueError(
                f"The llama.cpp {field} ID in {PIN} is not a verified 40-hex ID; "
                "record it before building or preparing local embeddings"
            )
    model = value["model"]
    if (
        not isinstance(model, dict)
        or set(model) != MODEL_FIELDS
        or not isinstance(model["repository"], str)
        or not re.fullmatch(r"[A-Za-z0-9][\w.-]{0,95}/[A-Za-z0-9][\w.-]{0,95}", model["repository"])
        or not isinstance(model["revision"], str)
        or not HEX40.fullmatch(model["revision"])
        or not isinstance(model["sha256"], str)
        or not HEX64.fullmatch(model["sha256"])
        or type(model["bytes"]) is not int
        or not 0 < model["bytes"] < 2**40
        or not isinstance(model["file"], str)
        or PurePosixPath(model["file"]).name != model["file"]
        or not model["file"].endswith(".gguf")
        or model["license"] != "Apache-2.0"
    ):
        raise ValueError("Invalid pinned embedding model")


def model_url(pin):
    model = pin["model"]
    return (
        f"https://huggingface.co/{model['repository']}/resolve/{model['revision']}/{model['file']}"
    )


def options(target):
    """Exact CMake options for one release target; nothing is left to the host."""
    arch, _, system = target.partition("-")
    values = dict(COMMON)
    if arch == "x86_64":
        values.update(HASWELL)
    elif arch != "aarch64":
        raise ValueError("The llama-server part supports arm64 or x86_64")
    if system == "apple-darwin":
        values.update(
            GGML_METAL="ON",
            GGML_METAL_EMBED_LIBRARY="ON",
            GGML_ACCELERATE="ON",
            GGML_BLAS="ON",
            GGML_BLAS_VENDOR="Apple",
            CMAKE_OSX_DEPLOYMENT_TARGET=MACOS_DEPLOYMENT_TARGET,
            CMAKE_OSX_ARCHITECTURES="arm64" if arch == "aarch64" else "x86_64",
            CMAKE_EXE_LINKER_FLAGS="",
        )
    elif system == "unknown-linux-gnu":
        # CPU only; the static C++ runtime keeps the glibc-only closure rule.
        values.update(
            GGML_METAL="OFF",
            GGML_BLAS="OFF",
            CMAKE_EXE_LINKER_FLAGS="-static-libstdc++ -static-libgcc",
        )
        if arch == "aarch64":
            values["GGML_CPU_ARM_ARCH"] = LINUX_ARM64_ARCH
    else:
        raise ValueError("The llama-server part supports macOS and GNU/Linux")
    return values


def cache_environment(inherited=None, *, system=None):
    """The release builder's admitted variables for a build outside the release driver.

    The opt-in E2E cache build drops build-altering overrides instead of refusing
    them, then builds with the same forwarded and fixed values as the release part.
    """
    from .build_environment import FIXED, FORWARD

    inherited = dict(os.environ if inherited is None else inherited)
    values = {key: value for key, value in inherited.items() if key in FORWARD}
    values.update(FIXED)
    if (platform.system() if system is None else system) == "Darwin":
        values["MACOSX_DEPLOYMENT_TARGET"] = MACOS_DEPLOYMENT_TARGET
    return values


def git_environment(env):
    """Ignore personal and system Git configuration; content IDs are verified anyway."""
    return dict(env) | {
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_TERMINAL_PROMPT": "0",
        "GIT_OPTIONAL_LOCKS": "0",
    }


def _git(directory, arguments, env, *, timeout=60):
    return run_process(
        ["git", "-C", str(directory), *arguments],
        env=env,
        text=True,
        check=True,
        timeout=timeout,
        limit=1024 * 1024,
    ).stdout.strip()


def fetch(directory, pin, env):
    """Fetch exactly the pinned commit by ID and verify its commit and tree IDs."""
    source = pin["llama_cpp"]
    environment = git_environment(env)
    run_process(
        ["git", "init", "--quiet", str(directory)], env=environment, check=True, timeout=60
    )
    run_process(
        [
            "git",
            "-C",
            str(directory),
            "fetch",
            "--quiet",
            "--depth",
            "1",
            "--no-tags",
            "--no-recurse-submodules",
            source["repository"],
            source["commit"],
        ],
        env=environment,
        check=True,
        timeout=FETCH_SECONDS,
        limit=4 * 1024 * 1024,
    )
    _git(directory, ["checkout", "--quiet", "--detach", source["commit"]], environment)
    verify_checkout(directory, pin, environment)


def verify_checkout(directory, pin, env):
    source = pin["llama_cpp"]
    if (
        _git(directory, ["rev-parse", "HEAD"], env) != source["commit"]
        or _git(directory, ["rev-parse", "HEAD^{tree}"], env) != source["tree"]
    ):
        raise ValueError("Fetched llama.cpp source differs from its pinned commit or tree")
    if _git(directory, ["status", "--porcelain", "--untracked-files=all"], env):
        raise ValueError("The pinned llama.cpp checkout is not clean")


def compilers(env):
    """Absolute compilers from the admitted PATH; CC/CXX overrides are never admitted."""
    found = {}
    for name in ("cc", "c++"):
        path = shutil.which(name, path=env.get("PATH", ""))
        if path is None:
            raise ValueError(f"The llama-server build needs {name} on the admitted PATH")
        found[name] = str(Path(path).absolute())
    return found


def identity(command, env):
    """The first output line only: bounded, printable and free of install paths."""
    output = run_process(command, env=env, text=True, check=True, timeout=30, limit=65536)
    lines = (output.stdout or output.stderr).splitlines()
    line = lines[0].strip() if lines else ""
    if not re.fullmatch(f"[ -~]{{1,{IDENTITY_BYTES}}}", line) or "/" in line:
        raise ValueError("Toolchain did not report a bounded identity line")
    return line


def toolchain(found, env):
    return {
        "cmake": identity(["cmake", "--version"], env),
        "compiler": {
            "c": identity([found["cc"], "--version"], env),
            "cxx": identity([found["c++"], "--version"], env),
        },
    }


def build(work, pin, target, env):
    """Configure and build only llama-server in a private tree; return it and its identity."""
    source, output = Path(work) / "source", Path(work) / "build"
    fetch(source, pin, env)
    flags = options(target)
    found = compilers(env)
    environment = git_environment(env)
    before = toolchain(found, env)
    run_process(
        [
            "cmake",
            "-S",
            str(source),
            "-B",
            str(output),
            *[f"-D{key}={value}" for key, value in sorted(flags.items())],
            f"-DCMAKE_C_COMPILER={found['cc']}",
            f"-DCMAKE_CXX_COMPILER={found['c++']}",
        ],
        env=environment,
        check=True,
        timeout=CONFIGURE_SECONDS,
        limit=32 * 1024 * 1024,
    )
    run_process(
        [
            "cmake",
            "--build",
            str(output),
            "--config",
            "Release",
            "--target",
            TARGET,
            "--parallel",
            str(JOBS),
        ],
        env=environment,
        check=True,
        timeout=BUILD_SECONDS,
        limit=64 * 1024 * 1024,
    )
    if toolchain(found, env) != before:
        raise ValueError("Toolchain identity changed during the llama-server build")
    verify_checkout(source, pin, environment)
    executable = output / "bin" / TARGET
    if not executable.is_file() or executable.is_symlink():
        raise ValueError("CMake did not produce the llama-server executable")
    llama_notices.check(source, output, NOTICES, metal=flags["GGML_METAL"] == "ON")
    record = {
        "repository": pin["llama_cpp"]["repository"],
        "commit": pin["llama_cpp"]["commit"],
        "tree": pin["llama_cpp"]["tree"],
        "cmake_options": flags,
        **before,
    }
    return executable, record


def version(path, *, env=None):
    """llama-server prints its version line on stderr; accept only the bounded form."""
    result = run_process(
        [str(path), "--version"], env=env, text=True, check=True, timeout=30, limit=65536
    )
    for line in (result.stderr + "\n" + result.stdout).splitlines():
        if VERSION.fullmatch(line.strip()):
            return line.strip()
    raise ValueError("llama-server did not report a version line")


def check_version(line, commit):
    match = VERSION.fullmatch(line) if isinstance(line, str) else None
    if match is None or not commit.startswith(match.group(1)):
        raise ValueError("llama-server --version does not report the pinned commit")


def _bounded(value):
    pattern = f"[ -~]{{1,{IDENTITY_BYTES}}}"
    return isinstance(value, str) and re.fullmatch(pattern, value) is not None


def check_record(record, target, *, pin=None):
    """A frozen record names exactly the pin and this target's CMake options."""
    pin = load() if pin is None else pin
    if (
        not isinstance(record, dict)
        or set(record) != {"repository", "commit", "tree", "cmake_options", "cmake", "compiler"}
        or record["repository"] != pin["llama_cpp"]["repository"]
        or record["commit"] != pin["llama_cpp"]["commit"]
        or record["tree"] != pin["llama_cpp"]["tree"]
        or record["cmake_options"] != options(target)
        or not _bounded(record["cmake"])
        or not isinstance(record["compiler"], dict)
        or set(record["compiler"]) != {"c", "cxx"}
        or not all(_bounded(value) for value in record["compiler"].values())
    ):
        raise ValueError("Build record differs from the pinned llama-server contract")


def product_version(source):
    """Hive's workspace version, bound to the admitted source snapshot."""
    data = (ROOT / "Cargo.toml").read_bytes()
    source.check_bytes("Cargo.toml", data)
    match = re.search(
        rb'^\[workspace\.package\]\r?\nversion = "([0-9][0-9A-Za-z.+-]{0,127})"\r?$', data, re.M
    )
    if match is None:
        raise ValueError("Cannot read the workspace product version")
    return match.group(1).decode()


def notices(source, pin, snapshot):
    """Notices of every third-party component compiled from the verified source tree."""
    records, text = source_notices(
        Path(source),
        NOTICES,
        vendor=VENDOR,
        not_compiled=NOT_COMPILED,
        version=pin["llama_cpp"]["commit"],
        origin=pin["llama_cpp"]["repository"],
    )
    return llama_notices.append_external(records, text, snapshot, ROOT)
