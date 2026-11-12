"""Sample a daemon's owned process trees. RSS includes shared resident pages."""

from release.process import run as run_process, ProcessFailure
from e2e.scripts.harness.worker import Worker
from release.files import atomic_write_text
from release.progress import event, finalizer

import datetime
import json
from release.files import AppendStream
from e2e.scripts.harness.io import json_lines
import os
import platform
import re
import statistics
import subprocess
import threading
import time
from pathlib import Path


def cpu_seconds(value):
    """ps TIME: cumulative process CPU, not elapsed lifetime or percent CPU."""
    if not re.fullmatch(r"(?:\d+-)?(?:\d+:)?\d+:\d+(?:\.\d+)?", value):
        raise RuntimeError(f"Invalid ps CPU time: {value}")
    days, clock = value.split("-", 1) if "-" in value else ("0", value)
    parts = clock.split(":")
    hours, minutes, seconds = parts if len(parts) == 3 else ("0", *parts)
    if int(minutes) >= 60 and len(parts) == 3 or float(seconds) >= 60:
        raise RuntimeError(f"Invalid ps CPU time: {value}")
    return int(days) * 86400 + int(hours) * 3600 + int(minutes) * 60 + float(seconds)


def process_table(cpu=False):
    columns = "pid=,ppid=,pgid=,rss=,lstart=," + ("time=," if cpu else "") + "comm="
    result = run_process(
        ["ps", "-axo", columns],
        capture_output=True,
        text=True,
        check=True,
        timeout=10,
        env=dict(os.environ, LC_ALL="C", TZ="UTC"),
    )
    rows = {}
    for line in result.stdout.splitlines():
        if not line.strip():
            continue
        fields = line.split(None, 10 if cpu else 9)
        if len(fields) != (11 if cpu else 10):
            raise RuntimeError("Incomplete process table row; RSS observation unavailable")
        pid, parent, group, rss = map(int, fields[:4])
        identity = " ".join(fields[4:9])
        started = (
            datetime.datetime.strptime(identity, "%a %b %d %H:%M:%S %Y")
            .replace(tzinfo=datetime.timezone.utc)
            .timestamp()
        )
        rows[pid] = {
            "pid": pid,
            "ppid": parent,
            "pgid": group,
            "rss_kib": rss,
            "start_time": started,
            "executable": Path(fields[-1]).name,
        }
        if cpu:
            rows[pid]["cpu_seconds"] = cpu_seconds(fields[9])
    return rows


def host_snapshot():
    commands = (
        [["sysctl", "-n", "hw.memsize"], ["sysctl", "vm.swapusage"], ["vm_stat"]]
        if platform.system() == "Darwin"
        else [["cat", "/proc/meminfo"]]
    )
    result = {}
    for command in commands:
        try:
            p = run_process(command, capture_output=True, text=True, timeout=10, check=True)
            result[" ".join(command)] = dict(complete=True, output=p.stdout.strip())
        except (OSError, subprocess.SubprocessError, ProcessFailure) as error:
            event("observation_failed", error=error, phase="observe", partial=True)
            result[" ".join(command)] = dict(complete=False, error=str(error))
    return result


class Sampler:
    def __init__(self, daemon_pid, output, max_rss_gib=32, cpu=False):
        self.daemon = daemon_pid
        self.output = Path(output)
        self.threshold = int(max_rss_gib * 1024 * 1024)
        self.stop_event = threading.Event()
        self.thread = None
        self.failure = None
        self.latest = None
        self.owned = {}
        self.daemon_start = None
        self.expect_shutdown = False
        self.cpu = cpu
        self.host_before = host_snapshot()

    def sample(self):
        before = time.monotonic_ns() if self.cpu else None
        table = process_table(cpu=self.cpu)
        after = time.monotonic_ns() if self.cpu else None
        daemon = table.get(self.daemon)
        if not daemon:
            if not self.expect_shutdown or self.daemon_start is None:
                raise RuntimeError("Daemon disappeared during memory sampling")
            daemon = dict(pid=self.daemon, start_time=self.daemon_start, rss_kib=0, exited=True)
        if self.daemon_start is None:
            self.daemon_start = daemon["start_time"]
        if daemon["start_time"] != self.daemon_start:
            raise RuntimeError("Daemon PID was reused")
        # Keep an observed descendant associated with its original launcher even
        # if its parent exits/reparents. PID+start_time prevents PID-reuse confusion.
        for row in table.values():
            if row["ppid"] == self.daemon:
                self.owned[(row["pid"], row["start_time"])] = (row["pid"], row["start_time"])
        while True:
            added = False
            for row in table.values():
                key = (row["pid"], row["start_time"])
                parent = table.get(row["ppid"])
                if key not in self.owned and parent:
                    owner = self.owned.get((parent["pid"], parent["start_time"]))
                    if owner is not None:
                        self.owned[key] = owner
                        added = True
            if not added:
                break
        processes = []
        for row in table.values():
            owner = self.owned.get((row["pid"], row["start_time"]))
            if owner is not None:
                processes.append({**row, "launcher_pid": owner[0], "launcher_start_time": owner[1]})
        sample = {
            "time": time.time(),
            "monotonic_time": time.monotonic(),
            "daemon": daemon,
            "processes": processes,
            "provider_tree_rss_kib": sum(p["rss_kib"] for p in processes),
            "daemon_rss_kib": daemon["rss_kib"],
        }
        sample["total_rss_kib"] = sample["provider_tree_rss_kib"] + daemon["rss_kib"]
        if self.cpu:
            sample.update(
                process_snapshot_before_ns=before,
                process_snapshot_after_ns=after,
                cpu_metric="ps TIME: coarse cumulative self CPU; non-atomic snapshot; short-lived processes may be absent",
            )
        self.latest = sample
        return sample

    def start(self):
        # Synchronous first sample establishes baseline/identity before spawning.
        first = self.sample()
        path = self.output / "memory.jsonl"
        if path.exists():
            raise RuntimeError("Memory capture already exists; use a new output directory")

        def run():
            try:
                with AppendStream.create_new(path, durable=False) as stream:
                    stream.append_jsonl(first)
                    while not self.stop_event.wait(1):
                        sample = self.sample()
                        stream.append_jsonl(sample)
                        if sample["total_rss_kib"] > self.threshold:
                            self.failure = f"RSS exceeded configured {self.threshold / 1024**2:g} GiB threshold"
                            event("observation_failed", phase="observe", partial=True)
                            break
            except Exception as error:
                self.failure = error
                raise

        self.thread = Worker(run, stop=self.stop_event)
        self.thread.start()

    def check(self):
        if self.thread is not None:
            self.thread.check()

    def stop(self):
        self.stop_event.set()
        with finalizer(self.save_host_snapshot):
            if self.thread:
                self.thread.join(timeout=15)

    def save_host_snapshot(self):
        after = host_snapshot()
        complete = all(
            item["complete"] for snapshot in (self.host_before, after) for item in snapshot.values()
        )
        atomic_write_text(
            self.output / "host-memory.json",
            json.dumps({"complete": complete, "before": self.host_before, "after": after}, indent=2)
            + "\n",
        )


def execution_owners(agents, events):
    owners = {}
    ready_times = []
    first_exit = float("inf")
    for a in agents:
        stream = events[a["id"]]
        starts = [e for e in stream if e["type"] == "provider_started"]
        ready = [e for e in stream if e["type"] == "session_ready"]
        if len(starts) != 1 or len(ready) != 1:
            raise RuntimeError("Stress measurement requires one provider execution per agent")
        e = starts[0]
        if e["data"]["pid"] in owners:
            raise RuntimeError(
                "This fixed 101-session measurement requires isolated providers; shared provider RSS cannot be attributed per agent"
            )
        owners[e["data"]["pid"]] = {
            "agent_id": a["id"],
            "role": a["role"],
            "spawn_before": e["data"]["spawn_before"],
            "spawn_after": e["data"]["spawn_after"],
        }
        ready_times.append(ready[0]["timestamp"])
        first_exit = min(
            first_exit, *(x["timestamp"] for x in stream if x["type"] == "provider_shutdown")
        )
    return owners, ready_times, first_exit


def summarize(path, agents, events):
    owners, ready_times, first_exit = execution_owners(agents, events)
    peak = None
    fully_loaded = []
    samples = 0
    timeline = []
    for sample in json_lines(path):
        samples += 1
        # RSS attribution includes only known execution-owned launcher subtrees.
        processes = [
            p
            for p in sample["processes"]
            if p["launcher_pid"] in owners
            and owners[p["launcher_pid"]]["spawn_before"]
            <= p["launcher_start_time"]
            <= owners[p["launcher_pid"]]["spawn_after"]
        ]
        launchers = {
            p["pid"]
            for p in processes
            if p["pid"] in owners
            and p["ppid"] == sample["daemon"]["pid"]
            and owners[p["pid"]]["spawn_before"]
            <= p["start_time"]
            <= owners[p["pid"]]["spawn_after"]
        }
        by_agent = {
            o["agent_id"]: sum(p["rss_kib"] for p in processes if p["launcher_pid"] == pid)
            for pid, o in owners.items()
        }
        row = {
            "time": sample["time"],
            "live_launchers": len(launchers),
            "provider_rss_kib": sum(by_agent.values()),
            "daemon_rss_kib": sample["daemon_rss_kib"],
            "per_agent_rss_kib": by_agent,
        }
        row["total_rss_kib"] = row["provider_rss_kib"] + row["daemon_rss_kib"]
        if peak is None or row["total_rss_kib"] > peak["total_rss_kib"]:
            peak = row
        # Event timestamps have second precision; leave a full-second margin.
        if len(launchers) == len(agents) and max(ready_times) + 1 <= row["time"] < first_exit:
            fully_loaded.append(row)
        timeline.append(row)
    return memory_summary(path, agents, peak, fully_loaded, samples, timeline)


def memory_summary(path, agents, peak, fully_loaded, samples, timeline):
    if not fully_loaded:
        raise RuntimeError("No sample observed all 101 loaded sessions alive together")
    intervals = [b["time"] - a["time"] for a, b in zip(timeline, timeline[1:])]
    gaps = [
        {"start": a["time"], "end": b["time"], "seconds": b["time"] - a["time"]}
        for a, b in zip(timeline, timeline[1:])
        if b["time"] - a["time"] > 5
    ]
    summary = {
        "metric": "sampled sum of RSS; includes shared pages; not unique physical RAM or remote model weights",
        "requested_sample_interval_seconds": 1,
        "sample_count": samples,
        "observed_interval_seconds": (
            {"min": min(intervals), "median": statistics.median(intervals), "max": max(intervals)}
            if intervals
            else None
        ),
        "sampling_gaps_over_5_seconds": gaps,
        "elapsed_seconds": timeline[-1]["time"] - timeline[0]["time"],
        "peak": peak,
        "all_agents_loaded_samples": len(fully_loaded),
        "all_agents_loaded_peak": max(fully_loaded, key=lambda r: r["total_rss_kib"]),
        "all_agents_loaded_mean_total_rss_kib": sum(r["total_rss_kib"] for r in fully_loaded)
        / len(fully_loaded),
        "baseline_total_rss_kib": timeline[0]["total_rss_kib"],
        "final_total_rss_kib": timeline[-1]["total_rss_kib"],
        "agents": len(agents),
        "limitations": "Observed peak only: sampling can miss peaks, especially during reported gaps. "
        "Process snapshots are not atomic; loaded mean is a sample mean, not a time-weighted mean.",
    }
    csv = "unix_time,live_launchers,provider_rss_kib,daemon_rss_kib,total_rss_kib\n"
    csv += "".join(
        f"{r['time']},{r['live_launchers']},{r['provider_rss_kib']},{r['daemon_rss_kib']},{r['total_rss_kib']}\n"
        for r in timeline
    )
    atomic_write_text(Path(path).with_suffix(".csv"), csv)
    return summary
