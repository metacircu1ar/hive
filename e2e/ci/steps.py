"""Current workflow steps, preserved separately from campaign ownership."""

import os
from pathlib import Path
import re
from release.process import run

PARTS = ("hive", "hive-core", "comb", "comb-llama", "hive-install")
CI_ENV = {"RUST_TEST_THREADS": "4", "CARGO_BUILD_JOBS": "4", "CARGO_TERM_COLOR": "never"}
VARIANT = re.compile(r"-\s*\{name:\s*([\w-]+),\s*package:\s*([\w-]+),\s*flags:\s*'([^']*)'\}")


def essential_steps():
    return [
        ("release Hive", "cargo build --release --locked -p hive", True),
        ("release Comb", "cargo build --release --locked -p comb", True),
        ("mock pairs", "python3 e2e/run.py mock pairs --agents 100", True),
        ("graph smoke", "python3 e2e/run.py graph --pages 11 --smoke", True),
    ]


def compatibility_baseline(checkout):
    """Compare with the first parent, or with HEAD for a root commit."""
    row = run(
        ["git", "rev-list", "--parents", "-n", "1", "HEAD"], cwd=checkout, text=True, check=True
    ).stdout.split()
    return row[1] if len(row) > 1 else row[0]


def distribution_steps():
    steps = []
    for part in PARTS:
        steps.append(
            (
                f"package {part}",
                f"python3 scripts/build_release.py --part {part} --output-dir dist/ci-parts/{part}"
                f" && python3 scripts/package_release.py --build-dir dist/ci-parts/{part}"
                " --output-dir dist/ci-assets",
                True,
            )
        )
    steps.extend(
        [
            (
                "packaging contracts",
                "python3 -m unittest discover -s tests -p test_release_packaging.py",
                True,
            ),
            (
                "installed release parts",
                "python3 scripts/check_release_install.py --assets dist/ci-assets"
                " --output-dir dist/ci-installed",
                True,
            ),
        ]
    )
    return steps


def variants(checkout, selected):
    found = [
        {"name": n, "package": p, "flags": f}
        for n, p, f in VARIANT.findall((checkout / ".github/workflows/rust.yml").read_text())
    ]
    if not found:
        raise RuntimeError("No variants found in .github/workflows/rust.yml")
    if selected:
        unknown = set(selected) - {v["name"] for v in found}
        if unknown:
            raise RuntimeError(f"Unknown variants: {sorted(unknown)}")
        found = [v for v in found if v["name"] in selected]
    return found


def variant_steps(variant, platform):
    """Workflow steps for one variant, as (name, shell command, required)."""
    name, package, flags = variant["name"], variant["package"], variant["flags"]
    select = f"-p {package} {flags}".strip()
    hive = name == "hive"
    parts = " ".join(PARTS)
    steps = []
    if hive:
        steps.append(
            (
                "freeze core",
                "cargo build --release --locked -p hive --no-default-features"
                " && mkdir -p dist/ci-core/release"
                " && cp target/release/hivectl target/release/hive-daemon dist/ci-core/release/",
                True,
            )
        )
    steps.append(("release build", f"cargo build --release --locked {select}", True))
    if hive:
        steps.append(
            (
                "freeze default",
                "mkdir -p dist/ci-hive/release"
                " && cp target/release/hivectl target/release/hive-daemon dist/ci-hive/release/",
                True,
            )
        )
    steps.append(
        ("clippy", f"cargo clippy --release --locked {select} --all-targets -- -D warnings", False)
    )
    steps.append(
        (
            "tests",
            f"cargo test --release --locked {select} --no-fail-fast"
            " -- --skip service::tests::embeddings::deadlines::adaptive::",
            False,
        )
    )
    if package == "comb":
        steps.append(
            (
                "adaptive timing",
                f"cargo test --release --locked -p comb {flags} --lib"
                " service::tests::embeddings::deadlines::adaptive:: -- --test-threads=1",
                False,
            )
        )
    if hive:
        steps += [
            (
                "capable core capable",
                'HIVE_DEFAULT_BINARY="$PWD/dist/ci-hive/release/hivectl"'
                ' HIVE_CORE_BINARY="$PWD/dist/ci-core/release/hivectl" cargo test --release --locked'
                " -p hive --test runtime capable_core_capable -- --ignored --nocapture",
                False,
            ),
            (
                "python product checks",
                "cargo build --release --locked -p hive"
                " && cmp target/release/hivectl dist/ci-hive/release/hivectl"
                " && cmp target/release/hive-daemon dist/ci-hive/release/hive-daemon"
                " && PYTHONPATH=scripts python3 -m unittest discover -s scripts/release -t scripts"
                " && python3 -m unittest discover -s tests -p 'test_release_packaging.py'"
                " && python3 -m unittest discover -s tests -p 'test_runtime_provider.py'",
                False,
            ),
            (
                "compatibility corpus",
                "cargo build --release --locked -p hive"
                " && cargo build --release --locked -p comb"
                " && python3 scripts/check_compatibility.py --current-dir target/release"
                " --output-dir dist/ci-compatibility",
                False,
            ),
            (
                "installed release parts",
                f"for part in {parts}; do"
                ' python3 scripts/build_release.py --part "$part" --output-dir "dist/ci-parts/$part"'
                ' && python3 scripts/package_release.py --build-dir "dist/ci-parts/$part"'
                " --output-dir dist/ci-assets || exit 1; done"
                " && python3 scripts/check_release_install.py --assets dist/ci-assets"
                " --output-dir dist/ci-installed",
                False,
            ),
        ]
    if name == "hive-install":
        steps.append(
            (
                "installer contracts",
                "cargo build --release --locked -p hive-install"
                " && python3 -m unittest discover -s tests -p 'test_installer_arguments.py'",
                False,
            )
        )
    return steps


def ci_env():
    """A runner-like environment: no inherited secrets, provider keys or tokens.
    HOME stays the user's own, as on hosted runners: Hive's launcher and service
    checks expect the account's real home."""
    keep = {
        key: os.environ[key]
        for key in ("PATH", "HOME", "USER", "LOGNAME", "LANG", "TMPDIR", "XDG_RUNTIME_DIR")
        if key in os.environ
    }
    home = Path(keep.get("HOME", "/"))
    keep["CARGO_HOME"] = os.environ.get("CARGO_HOME", str(home / ".cargo"))
    keep["RUSTUP_HOME"] = os.environ.get("RUSTUP_HOME", str(home / ".rustup"))
    # A capable terminal type: tmux refuses to start under TERM=dumb.
    return {**keep, **CI_ENV, "SHELL": "/bin/bash", "TERM": "xterm-256color"}
