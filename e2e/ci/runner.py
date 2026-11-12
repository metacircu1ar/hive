"""Execute release workflow steps and return a provisional campaign result."""

import re
import shlex
import stat
import time
from pathlib import Path

from e2e.ci.steps import (
    ci_env,
    compatibility_baseline,
    distribution_steps,
    essential_steps,
    variant_steps,
    variants,
)
from e2e.scripts.harness.scenario import run
from release.cancellation import check
from release.files import atomic_copy, atomic_write_json, create_directory, raw_log
from release.progress import phase

STEP_OUTPUT_BYTES = 64 * 1024**2
STEP_SECONDS = 3600
SHELL_PREFIX = "trap 'exit 130' INT; trap 'exit 143' TERM; trap 'exit 129' HUP\n"
RESULT = re.compile(r"test result: \w+\. (\d+) passed; (\d+) failed")


def summarize(log):
    passed = failed = 0
    for match in RESULT.finditer(log):
        passed += int(match.group(1))
        failed += int(match.group(2))
    failures = sorted(
        {
            line.split()[1]
            for line in log.splitlines()
            if line.startswith("test ") and line.endswith(" FAILED")
        }
    )
    return dict(tests_passed=passed, tests_failed=failed, failed_tests=failures[:100])


def copy_tree(source, destination):
    for path in sorted(source.rglob("*")):
        check()
        if path.is_symlink():
            raise ValueError("Release asset export refuses symbolic links")
        target = destination / path.relative_to(source)
        if path.is_dir():
            create_directory(target, exist_ok=True)
        else:
            atomic_copy(path, target, mode=stat.S_IMODE(path.stat().st_mode))


class Runner:
    def __init__(
        self,
        platform,
        checkout,
        output,
        *,
        selected=None,
        full=False,
        distribution=False,
        inherited=(),
        installed_assets=None,
    ):
        if installed_assets is not None and (
            not full or selected or distribution or not platform.startswith("linux-")
        ):
            raise ValueError("Prebuilt assets require a full Linux VM workflow")
        self.platform, self.checkout, self.output = platform, checkout, output
        create_directory(output / "logs", exist_ok=True)
        self.env = ci_env()
        self.results = []
        self.selected, self.full, self.distribution = selected, full, distribution
        self.inherited = inherited
        self.installed_assets = Path(installed_assets) if installed_assets is not None else None
        if self.installed_assets is not None and (
            self.installed_assets.is_symlink() or not self.installed_assets.is_dir()
        ):
            raise ValueError("Prebuilt VM assets must be an existing directory")

    def step(self, variant, name, command):
        import json

        started = time.monotonic()
        print(
            json.dumps(dict(platform=self.platform, variant=variant, step=name, status="started")),
            flush=True,
        )
        path = self.output / "logs" / (variant + "--" + name.replace(" ", "-") + ".log")
        with phase("verify"), raw_log(path) as log:
            log.write(("$ " + command + "\n").encode())
            done = run(
                # Bash defers these traps until its foreground command exits.
                # Keep the supervised leader alive while cooperative Python
                # owners retire children in their separate process groups.
                ["bash", "-c", SHELL_PREFIX + command],
                cwd=self.checkout,
                env=self.env,
                check=False,
                timeout=STEP_SECONDS,
                stdout=log,
                stderr=log,
                limit=STEP_OUTPUT_BYTES,
                pass_fds=self.inherited,
            )
        result = dict(
            variant=variant,
            step=name,
            returncode=done.returncode,
            seconds=round(time.monotonic() - started, 2),
            log=str(path.relative_to(self.output)),
        )
        if name in ("tests", "adaptive timing", "capable core capable"):
            result.update(summarize(path.read_text(errors="replace")))
        self.results.append(result)
        atomic_write_json(self.output / "steps.json", self.results)
        print(json.dumps(dict(platform=self.platform, **result)), flush=True)
        return done.returncode == 0

    def skip(self, variant, name, reason):
        self.results.append(dict(variant=variant, step=name, returncode=None, skipped=reason))

    def export_assets(self):
        source = self.installed_assets or self.checkout / "dist/ci-assets"
        if not source.is_dir():
            return []
        destination = self.output / "assets"
        create_directory(destination, exist_ok=True)
        copy_tree(source, destination)
        return sorted(
            str(path.relative_to(destination)) for path in destination.rglob("*") if path.is_file()
        )

    def run(self):
        commit = run(
            ["git", "rev-parse", "HEAD"],
            cwd=self.checkout,
            text=True,
            check=True,
            pass_fds=self.inherited,
        ).stdout.strip()
        started = int(time.time())
        validation = (
            "distribution"
            if self.distribution
            else "ci"
            if self.full or self.selected
            else "essential"
        )
        if validation != "essential":
            self.step("workspace", "format", "cargo fmt --all --check")
            self.step(
                "workspace",
                "format included files",
                "rustfmt --edition 2024 --check tests/cli/fixture.rs tests/setup_cli/uninstall.rs",
            )
            self.step(
                "workspace",
                "compatibility gate",
                "PYTHONPATH=scripts python3 -m release.compatibility_gate --baseline "
                + compatibility_baseline(self.checkout),
            )
        if self.distribution:
            groups = [("distribution", distribution_steps())]
        elif validation == "essential":
            groups = [("essential", essential_steps())]
        else:
            groups = [
                (variant["name"], variant_steps(variant, self.platform))
                for variant in variants(self.checkout, self.selected)
            ]
        for variant, steps in groups:
            for index, (name, command, required) in enumerate(steps):
                if self.installed_assets is not None and variant == "hive" and name == "installed release parts":
                    command = (
                        "python3 scripts/check_release_install.py --assets "
                        + shlex.quote(str(self.installed_assets))
                        + " --output-dir dist/ci-installed"
                    )
                if not self.step(variant, name, command) and required:
                    for later, _, _ in steps[index + 1 :]:
                        self.skip(variant, later, name + " failed")
                    break
        assets = self.export_assets()
        failed = [row for row in self.results if row.get("returncode") not in (0, None)]
        return dict(
            platform=self.platform,
            profile="release",
            commit=commit,
            validation=validation,
            started_at=started,
            finished_at=int(time.time()),
            passed=not failed,
            failed_steps=[row["variant"] + ": " + row["step"] for row in failed],
            assets=assets,
            asset_origin="prebuilt VM assets" if self.installed_assets is not None else "workflow build",
            steps=self.results,
        )
