"""Check actual compiled sources/headers for uncovered in-tree license notices.

Vendor components have their separate exact notice inventory. Compiler-generated
Make depfiles cover the non-vendor translation units and headers. Embedded Metal
sources/headers are also scanned because their generated assembly hides them from
the compiler's depfile. New attributions require a reviewed notice declaration.
"""

from pathlib import Path
import hashlib
import re
import shlex

from .licenses import EXCERPT_LINES
from .native_evidence import current

EXTERNAL_NOTICES = (
    (
        "DRY sampler port", "scripts/release/notices/dry-permission.txt",
        "https://github.com/ggml-org/llama.cpp/pull/9702#discussion_r1805786189",
    ),
    (
        "DeepSeek image preprocessor port", "scripts/release/notices/deepseek.txt",
        ("https://huggingface.co/deepseek-ai/DeepSeek-V4-Flash-Vision-Exp/resolve/"
         "6821d6ad3681a4b137b066b76094fa82ebd0a380/LICENSE"),
    ),
    (
        "OpenVINO SVE adaptation", "scripts/release/notices/openvino.txt",
        ("https://raw.githubusercontent.com/openvinotoolkit/openvino/"
         "74331408dd41701e01e14a5874a41e744f0a2f03/LICENSE"),
    ),
    (
        "OpenVINO SVE adaptation", "scripts/release/notices/openvino-attribution.txt",
        ("https://raw.githubusercontent.com/openvinotoolkit/openvino/"
         "74331408dd41701e01e14a5874a41e744f0a2f03/"
         "src/plugins/intel_cpu/src/nodes/kernels/scaled_attn/common.hpp"),
    ),
    (
        "ggllm.cpp tokenizer adaptation", "scripts/release/notices/ggllm.txt",
        ("https://raw.githubusercontent.com/cmp-nct/ggllm.cpp/"
         "0dc65eb735f90f82143ead9dedb66e73fa25d114/LICENSE"),
    ),
    (
        "Arm optimized-routines adaptation", "scripts/release/notices/arm-optimized-routines.txt",
        ("https://raw.githubusercontent.com/ARM-software/optimized-routines/"
         "20859d3fd9e797dde592c489c8ef7d383f778620/LICENSE"),
    ),
    (
        "Pillow resampling adaptation", "scripts/release/notices/pillow.txt",
        ("https://raw.githubusercontent.com/python-pillow/Pillow/"
         "ed94d78c826c96acb912ec537d539c4410cb9b4a/LICENSE"),
    ),
)

# Third-party code compiled into the llama-server target at the pinned commit.
# Review this with every pin change: the build refuses a missing notice file and a
# vendor directory that is classified neither here nor in NOT_COMPILED. A second
# element names a marker; the notice is then the bounded excerpt starting there.
NOTICES = (
    ("llama.cpp and ggml", "MIT", (("LICENSE", None),)),
    (
        "DRY sampler port", "MIT",
        (
            ("src/llama-sampler.cpp", "Ported from Koboldcpp, original PR:"),
            ("include/llama.h", "porting Koboldcpp implementation authored by pi6am:"),
            ("LICENSE", None),
        ),
    ),
    (
        "DeepSeek image preprocessor port", "MIT",
        (("tools/mtmd/mtmd-image.cpp",
          "port of load_image / safe_resize / solve_resize_ratio / grid_tokens"),),
    ),
    (
        # The current portable targets do not enable SVE. Retain its upstream
        # licence too, rather than exempting the borrowed code in a built header.
        "OpenVINO SVE adaptation", "Apache-2.0",
        (("ggml/src/ggml-cpu/vec.h", "Below function was borrowed from the GitHub repository:"),),
    ),
    (
        "ggllm.cpp tokenizer adaptation",
        "MIT",
        (("src/llama-vocab.cpp", "adapted from https://github.com/cmp-nct/ggllm.cpp"),),
    ),
    (
        "Arm optimized-routines adaptation",
        "MIT",
        (("ggml/src/ggml-cpu/vec.h", "adapted from arm limited optimized routine"),),
    ),
    (
        "Pillow resampling adaptation",
        "MIT-CMU",
        (("tools/mtmd/mtmd-image.cpp", "Adapted from https://github.com/python-pillow/Pillow/"),),
    ),
    (
        "public-domain Z-algorithm adaptation",
        "MIT AND LicenseRef-Public-Domain",
        (
            ("src/llama-sampler.cpp", "This algorithm is not currently documented on Wikipedia"),
            ("LICENSE", None),
        ),
    ),
    (
        "public-domain base64",
        "Unlicense",
        (("common/base64.hpp",
          "This is free and unencumbered software released into the public domain."),),
    ),
    (
        "llamafile sgemm",
        "MIT",
        (("ggml/src/ggml-cpu/llamafile/sgemm.cpp", "Copyright 2024 Mozilla Foundation"),),
    ),
    (
        "YaRN",
        "MIT",
        (
            ("ggml/src/ggml-cpu/ops.cpp", "MIT licensed. Copyright (c) 2023 Jeffrey Quesnelle"),
            ("ggml/src/ggml-metal/kernels/rope.metal", "Copyright (c) 2023 Jeffrey Quesnelle"),
            ("LICENSE", None),
        ),
    ),
    ("cpp-httplib", "MIT", (("vendor/cpp-httplib/LICENSE", None),)),
    ("nlohmann/json", "MIT", (("licenses/LICENSE-jsonhpp", None),)),
    (
        "hash helpers",
        "MIT AND BSD-2-Clause AND LicenseRef-Public-Domain",
        (
            ("vendor/hash/rotate-bits/LICENSE.md", None),
            ("vendor/hash/sha1/sha1.c", "SHA-1 in C"),
            ("vendor/hash/sha256/LICENSE", None),
            ("vendor/hash/xxhash/LICENSE", None),
        ),
    ),
    (
        "stb_image",
        "MIT OR Unlicense",
        (("vendor/stb/stb_image.h", "This software is available under 2 licenses"),),
    ),
    (
        "miniaudio",
        "MIT-0 OR Unlicense",
        (
            (
                "vendor/miniaudio/miniaudio.h",
                "This software is available as a choice of the following licenses",
            ),
        ),
    ),
)
VENDOR = {
    "CMakeLists.txt": "llama.cpp and ggml",
    "cpp-httplib": "cpp-httplib",
    "nlohmann": "nlohmann/json",
    "hash": "hash helpers",
    "stb": "stb_image",
    "miniaudio": "miniaudio",
}
# LLAMA_SUBPROCESS=OFF and MTMD_VIDEO=OFF exclude both header consumers
# (common/subproc.h and tools/mtmd/mtmd-helper.cpp). The interface target alone
# contributes no source, so its subprocess implementation is not compiled.
NOT_COMPILED = frozenset({"sheredom"})


HINT = re.compile(
    r"\bcopyright\b(?!\s+(?:notice|holders?|laws?|interest)\b)"
    r"|\bSPDX-License-Identifier\b|\bpublic domain\b|\bMIT licensed\b"
    r"|\b(?:MIT|BSD|Apache)[^\n]{0,40}\blicen[cs]e\b"
    r"|\blicen[cs]ed under\b|\(c\)\s*[0-9]{4}|permission is hereby granted"
    r"|\b(?:L|A)?GPL\b|all rights reserved|adapted from"
    r"|\bport(?:ed|ing)? (?:from|of)\b|\bporting\b|\bborrowed from\b",
    re.IGNORECASE,
)


def compiled_paths(source, build):
    source, build = Path(source).resolve(), Path(build)
    paths = set()
    depfiles = list(build.rglob("*.o.d"))
    if not depfiles:
        raise ValueError("Missing compiled-source dependency evidence for native notices")
    for depfile in depfiles:
        words = shlex.split(depfile.read_text().replace("\\\n", " "))
        if not words or not words[0].endswith(":"):
            raise ValueError("Invalid native compiler dependency evidence")
        for word in words[1:]:
            path = Path(word)
            if not path.is_absolute():
                path = build / path
            path = path.resolve(strict=True)
            if path.is_relative_to(source) and not path.is_relative_to(source / "vendor"):
                paths.add(path)
    if not paths:
        raise ValueError("Native compiler dependencies contain no pinned source")
    return paths


def coverage(source, declarations):
    covered = {}
    for _, _, files in declarations:
        for relative, marker in files:
            path = source / relative
            lines = path.read_text().splitlines()
            start = 0 if marker is None else next(
                (i for i, line in enumerate(lines) if marker in line), None
            )
            if start is None:
                raise ValueError(f"License marker not found: {relative}")
            end = len(lines) if marker is None else start + EXCERPT_LINES
            covered.setdefault(path.resolve(), []).append(range(start, end))
            if marker is not None:
                # One retained notice also covers identical repeated attribution
                # lines. Other later notices still require their own declaration.
                covered[path.resolve()].extend(
                    range(i, i + 1) for i, line in enumerate(lines)
                    if i > start and line.strip() == lines[start].strip()
                )
    return covered


def check(source, build, declarations, *, metal):
    source = Path(source).resolve()
    paths = compiled_paths(source, build)
    if metal:
        directory = source / "ggml/src/ggml-metal"
        paths.update(directory.rglob("*.metal"))
        paths.update(directory.rglob("*.h"))
    covered = coverage(source, declarations)
    missing = []
    for path in sorted(paths):
        for index, line in enumerate(path.read_text().splitlines()):
            if (
                HINT.search(line)
                and "ggml authors" not in line.lower()
                and not own_adaptation(source, path, line)
                and not any(index in span for span in covered.get(path.resolve(), []))
            ):
                missing.append(f"{path.relative_to(source)}:{index + 1}")
    if missing:
        error = ValueError(f"Unreviewed in-tree license notice: {missing[0]}")
        sink = current()
        if sink is not None:
            try:
                sink.retain(
                    ("Unreviewed in-tree license notices:\n" + "\n".join(missing) + "\n").encode()
                )
            except (OSError, ValueError, RuntimeError) as secondary:
                error.cleanup_errors = (secondary,)
        raise error


def own_adaptation(source, path, line):
    if (
        path.relative_to(source).as_posix() == "ggml/src/ggml-backend.cpp"
        and line.strip() == "// Note: porting this file to C++ is a work in progress"
    ):
        # A language conversion of this same project file, not external code.
        return True
    # Both named source models belong to this same verified ggml tree and its
    # root MIT notice; this is an internal adaptation, not an omitted component.
    return (
        path.relative_to(source).as_posix() == "src/models/dots3note.cpp"
        and line.strip() == (
            "// note: code adapted from deepseek32.cpp (DSA indexer + absorbed MLA) "
            "and step35.cpp (head-wise output gate)"
        )
        and all((source / "src/models" / name).is_file()
                for name in ("deepseek32.cpp", "step35.cpp"))
    )


def append_external(records, text, snapshot, root):
    """Retain the original licenses for adaptations whose notices are not vendored.

    The fixed upstream URLs document provenance; builds only read the reviewed
    local copies. Every consumed byte is bound to the product source snapshot.
    Arm's MIT alternative is selected; the whole upstream license is retained.
    """
    indexed = {record["name"]: record for record in records}
    sections = [text]
    for name, relative, origin in EXTERNAL_NOTICES:
        data = (root / relative).read_bytes()
        snapshot.check_bytes(relative, data)
        indexed[name]["notices"].append(
            {"file": relative, "sha256": hashlib.sha256(data).hexdigest()}
        )
        sections.append(
            f"\n--- {name}: original license ---\nSource: {origin}\n{data.decode('utf-8')}\n"
        )
    return records, "".join(sections)
