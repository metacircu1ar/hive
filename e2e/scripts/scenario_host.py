"""Low-frequency host observations for noisy local performance comparisons."""

from release.process import run as run_process, ProcessFailure

import json
from release.files import append_jsonl_record
import os
import platform
import subprocess
import time
from pathlib import Path


class HostObserver:
    def __init__(self, output):
        self.path = Path(output) / "host-load.jsonl"
        self.last = None
        self.seconds = 0.0

    def sample(self):
        began = time.monotonic()
        if self.last is not None and began - self.last < 60:
            return
        row = dict(monotonic_ns=time.monotonic_ns(), wall_ns=time.time_ns())
        try:
            row["load_average"] = os.getloadavg()
            result = run_process(
                ["ps", "-A", "-o", "%cpu=,comm="],
                capture_output=True,
                text=True,
                timeout=2,
                check=True,
                env=dict(os.environ, LC_ALL="C"),
            )
            processes = []
            for line in result.stdout.splitlines():
                cpu, name = line.strip().split(None, 1)
                processes.append(dict(cpu_percent=float(cpu), executable=Path(name).name))
            row["top_cpu"] = sorted(processes, key=lambda p: p["cpu_percent"], reverse=True)[:15]
        except (OSError, ValueError, subprocess.SubprocessError, ProcessFailure) as error:
            row["load_error"] = str(error)
        if platform.system() == "Darwin":
            try:
                result = run_process(
                    ["pmset", "-g", "therm"],
                    capture_output=True,
                    text=True,
                    timeout=2,
                    check=True,
                )
                row["thermal"] = result.stdout.strip()
            except (OSError, subprocess.SubprocessError, ProcessFailure) as error:
                row["thermal_error"] = str(error)
        row["observation_seconds"] = time.monotonic() - began
        append_jsonl_record(self.path, row, durable=False)
        self.seconds += row["observation_seconds"]
        self.last = began
