"""Process-tree RSS and optional macOS physical footprint."""

import ctypes
import platform
import statistics
import time
from release.cancellation import pause

from e2e.scripts.harness.checks import one, require
from release.files import atomic_write_json
from e2e.scripts.harness.io import json_lines
from e2e.scripts.scenario_memory import Sampler, process_table


class Usage(ctypes.Structure):
    # sys/resource.h, rusage_info_v0. Sizes are bytes; timestamps are mach ticks.
    _fields_ = [("uuid", ctypes.c_uint8 * 16)] + [
        (name, ctypes.c_uint64)
        for name in (
            "user_time",
            "system_time",
            "pkg_idle_wkups",
            "interrupt_wkups",
            "pageins",
            "wired_size",
            "resident_size",
            "phys_footprint",
            "proc_start_abstime",
            "proc_exit_abstime",
        )
    ]


class FootprintSampler(Sampler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.lib = None
        if platform.system() == "Darwin":
            self.lib = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
            self.lib.proc_pid_rusage.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]
            self.lib.proc_pid_rusage.restype = ctypes.c_int
        self.kernel_starts = {}

    def measure(self, row):
        if self.lib is None:
            return dict(complete=False, reason="native_footprint_unavailable_on_this_platform")
        if row.get("exited"):
            return dict(complete=True, bytes=0, exited=True)
        usage = Usage()
        if self.lib.proc_pid_rusage(row["pid"], 0, ctypes.byref(usage)) != 0:
            return dict(complete=False, errno=ctypes.get_errno())
        identity = (row["pid"], row["start_time"])
        previous = self.kernel_starts.setdefault(identity, usage.proc_start_abstime)
        if previous != usage.proc_start_abstime or usage.proc_exit_abstime:
            return dict(complete=False, reason="process_identity_changed_or_exited")
        return dict(
            complete=True, bytes=usage.phys_footprint, kernel_start=usage.proc_start_abstime
        )

    def verify_gone(self, timeout=45):
        deadline = time.monotonic() + timeout
        while True:
            rows = process_table()
            remaining = [
                pid
                for pid, started in list(self.owned)
                if pid in rows and rows[pid]["start_time"] == started
            ]
            daemon = rows.get(self.daemon)
            daemon_gone = not daemon or (
                self.daemon_start is not None and daemon["start_time"] != self.daemon_start
            )
            if not remaining and daemon_gone:
                break
            require(time.monotonic() < deadline, "Owned processes remain after daemon shutdown")
            pause(0.1)
        atomic_write_json(
            self.output / "process-cleanup.json",
            dict(confirmed=True, observed_descendants=len(self.owned), remaining=[]),
        )

    def sample(self):
        sample = super().sample()
        rows = [sample["daemon"], *sample["processes"]]
        for row in rows:
            row["footprint"] = self.measure(row)
        complete = all(row["footprint"]["complete"] for row in rows)
        sample["footprint_complete"] = complete
        sample["footprint_available"] = self.lib is not None
        sample["total_footprint_bytes"] = (
            sum(row["footprint"]["bytes"] for row in rows) if complete else None
        )
        return sample


def distribution(values):
    require(bool(values), "Cannot summarize empty observations")
    ordered = sorted(values)
    return dict(
        min=min(values),
        median=statistics.median(values),
        p95=ordered[max(0, (95 * len(ordered) + 99) // 100 - 1)],
        max=max(values),
        mean=statistics.mean(values),
    )


def shared_owners(evidence):
    owners = {}
    for agent in evidence["agents"]:
        start = one(
            [e for e in evidence["events"][agent["id"]] if e["type"] == "provider_started"],
            "provider execution",
        )["data"]
        identity = (start["provider_id"], start["pid"], start["spawn_before"], start["spawn_after"])
        owner = owners.setdefault(
            start["pid"],
            dict(
                identity=identity,
                roles=[],
                spawn_before=start["spawn_before"],
                spawn_after=start["spawn_after"],
                agents=[],
            ),
        )
        require(owner["identity"] == identity, "Provider PID reuse invalidates hold attribution")
        owner["agents"].append(agent["id"])
        if agent["role"] not in owner["roles"]:
            owner["roles"].append(agent["role"])
    return owners


def summarize(path, evidence, hold):
    owners = shared_owners(evidence)
    samples = list(json_lines(path))
    full = []
    for sample in samples:
        if not hold[0] <= sample["monotonic_time"] <= hold[1]:
            continue
        present = {
            p["pid"]
            for p in sample["processes"]
            if p["pid"] in owners
            and owners[p["pid"]]["spawn_before"]
            <= p["start_time"]
            <= owners[p["pid"]]["spawn_after"]
        }
        if len(present) != len(owners):
            continue
        full.append(sample)
    require(len(full) >= 3, "Fewer than three complete all-agent hold samples")
    complete = [s for s in samples if s["footprint_complete"]]
    footprint_hold = [s for s in full if s["footprint_complete"]]
    available = any(s["footprint_available"] for s in samples)
    require(
        not available or len(footprint_hold) >= 3,
        "Fewer than three complete footprint hold samples",
    )
    by_process = {}
    for pid, owner in owners.items():

        def values(selected, *, footprint=False, owner=owner, pid=pid):
            return [
                sum(
                    (p["footprint"]["bytes"] / 1024**2 if footprint else p["rss_kib"] / 1024)
                    for p in s["processes"]
                    if p["launcher_pid"] == pid
                    and owner["spawn_before"] <= p["launcher_start_time"] <= owner["spawn_after"]
                )
                for s in selected
            ]

        by_process[str(pid)] = dict(
            roles=sorted(owner["roles"]),
            agents=owner["agents"],
            rss_mib=distribution(values(full)),
            footprint_mib=distribution(values(footprint_hold, footprint=True))
            if footprint_hold
            else None,
        )
    return dict(
        note="Sampled process-tree RSS double-counts shared pages; macOS physical footprint "
        "includes kernel-accounted/compressed memory and is not a unique page union. "
        "Provider model weights run remotely. Baseline includes daemon only.",
        provider_processes=len(owners),
        footprint_available=available,
        conversations_per_process={str(pid): len(owner["agents"]) for pid, owner in owners.items()},
        samples=len(samples),
        partial_footprint_samples=len(samples) - len(complete),
        interval_seconds=distribution(
            [b["monotonic_time"] - a["monotonic_time"] for a, b in zip(samples, samples[1:])]
        ),
        peak_total_rss_gib=max(s["total_rss_kib"] for s in samples) / 1024**2,
        peak_complete_total_footprint_gib=max(s["total_footprint_bytes"] for s in complete)
        / 1024**3
        if complete
        else None,
        full_hold_samples=len(full),
        full_hold_total_rss_gib=distribution([s["total_rss_kib"] / 1024**2 for s in full]),
        full_hold_total_footprint_gib=distribution(
            [s["total_footprint_bytes"] / 1024**3 for s in footprint_hold]
        )
        if footprint_hold
        else None,
        process_trees=by_process,
        baseline_rss_mib=samples[0]["total_rss_kib"] / 1024,
        final_rss_mib=samples[-1]["total_rss_kib"] / 1024,
    )
