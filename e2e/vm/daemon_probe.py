"""Run doctor against a short-lived in-guest user service without installing Hive."""

import json
from pathlib import Path
import time
import uuid

from e2e.paths import product_default
from release.cancellation import pause
from release.files import atomic_write_json
from release.owned_process import spawn
from release.process import run
from release.progress import entry, finalizer
from release.temporary import directory


def main():
    unit = "hive-vm-probe-" + uuid.uuid4().hex
    with directory(prefix="hive-vm-doctor-") as root:
        config = root / "config.yaml"
        atomic_write_json(
            config,
            dict(
                auth_profiles={
                    "codex": dict(provider="codex", method="subscription"),
                    "claude": dict(provider="claude", method="subscription"),
                }
            ),
        )
        shared = [
            "--config",
            str(config),
            "--state-dir",
            str(root / "state"),
            "--codex-bin",
            str(Path.home() / "providers/bin/codex"),
            "--claude-bin",
            str(Path.home() / "providers/bin/claude"),
        ]
        ctl = [str(product_default("target/release/hivectl")), *shared]
        command = [
            "systemd-run",
            "--user",
            "--wait",
            "--pipe",
            "--collect",
            "--unit",
            unit,
            "--property=RuntimeMaxSec=300",
            str(product_default("target/release/hive-daemon")),
            *shared,
        ]
        with spawn(command, limit=4 * 1024**2) as owner:
            with finalizer(
                lambda: run(["systemctl", "--user", "stop", unit], check=True, timeout=30)
            ):
                until = time.monotonic() + 60
                while True:
                    if owner.exited():
                        raise RuntimeError("The transient doctor service exited before readiness")
                    status = run([*ctl, "daemon", "status"], check=False, text=True, timeout=5)
                    if status.returncode == 0:
                        break
                    if time.monotonic() >= until:
                        raise TimeoutError("The transient doctor service did not become ready")
                    pause(0.1)
                done = run([*ctl, "doctor"], check=False, text=True, timeout=120)
                report = json.loads(done.stdout)
                print(
                    "HIVE_VM_JSON " + json.dumps(dict(returncode=done.returncode, report=report)),
                    flush=True,
                )


if __name__ == "__main__":
    raise SystemExit(entry(main, name="verify"))
