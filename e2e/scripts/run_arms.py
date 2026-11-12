"""Compare two release builds with alternating runs and storage observations.

Each run is a cool-down gap, an I/O canary, one arm and a second canary; the
daemon is down during gaps and canaries because each arm starts and stops its
own. Arms alternate ABBA. A run is `slow` when either canary falls below half
of the session reference (the canary after the first gap) or below an explicit
absolute floor. Pairs are compared only when both runs share a mode; runs
continue until enough same-mode pairs exist or the pair cap is reached. Every
run is reported, whatever its mode or outcome.

`--probe-interval` samples one 16 KiB write and F_FULLFSYNC per interval while
each arm runs , because the canaries miss short storage stalls. Each run keeps its samples in probe.json, and a probe sync over
100 ms also marks the run slow: Long flushes mark the storage observation as slow.

`--barrier-delay-ms` sets HIVE_BARRIER_DELAY_MS for every daemon . Both binaries must be `barrier-delay` measurement builds, and each passed
run must show, in the pair driver's startup.json, that its daemon logged the same
delay. Without it the variable is removed, so a shell value
cannot leak into a run.

Driver arguments follow `--` and must not include --binary or --output-dir.
"""

from release.cancellation import check, pause
from release.process import run as run_process
from e2e.scripts.harness.scenario import run as run_scenario, SCENARIO_STOP_GRACE

#!/usr/bin/env python3
import sys as _helper_sys
from pathlib import Path as _HelperPath
from release.files import atomic_write_text, create_directory
from release.progress import entry, phase, finalizer
from release.owned_process import spawn
from release.process import ProcessFailure
from e2e.paths import ROOT
from e2e.scripts.harness.io import json_lines
from e2e.scripts.harness.hashing import sha256
import uuid
import signal
import argparse
import contextlib
import fcntl
import json
import os
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

PAIR_DRIVER = ROOT / "e2e/run.py"
CANARY_BYTES = 32 * 1024 * 1024
RECORD_BYTES = 16 * 1024
SYNC_BYTES = 1024 * 1024
METRICS = ("pairs_per_second", "trace_pairs_per_second", "sampled_peak_daemon_rss_mib")
# A probe sync this slow is a device flush stall .
PROBE_SLOW_MS = 100


def parse_args(argv=None):
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--arm",
        action="append",
        required=True,
        metavar="NAME=BINARY",
        help="Exactly two arms, each a name and a release binary",
    )
    parser.add_argument(
        "--pairs", type=int, required=True, help="Same-mode ABBA pairs required before stopping"
    )
    parser.add_argument("--max-pairs", type=int, required=True, help="Pair cap")
    parser.add_argument(
        "--cooldown", type=int, default=120, help="Seconds with no daemon before each run's canary"
    )
    parser.add_argument(
        "--probe-interval",
        type=float,
        help="Seconds between F_FULLFSYNC probe samples during each arm",
    )
    parser.add_argument(
        "--barrier-delay-ms",
        type=int,
        help="Ledger barrier delay for both arms; requires barrier-delay builds",
    )
    parser.add_argument(
        "--canary-floor-mibs",
        type=float,
        help="Absolute MiB/s below which a canary marks the run slow",
    )
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument(
        "driver", nargs=argparse.REMAINDER, help="-- then run_mock_ping_pong_pair.py arguments"
    )
    args = parser.parse_args(argv)
    if len(args.arm) != 2:
        parser.error("Pass exactly two --arm NAME=BINARY values")
    arms = []
    for value in args.arm:
        name, separator, binary = value.partition("=")
        path = Path(binary)
        if not separator or not name or not path.is_file() or "release" not in path.parts:
            parser.error(f"Invalid arm {value!r}: use NAME=/path/to/release/hivectl")
        arms.append((name, path.resolve()))
    if arms[0][0] == arms[1][0]:
        parser.error("Arm names must differ")
    args.arms = arms
    if not 1 <= args.pairs <= args.max_pairs <= 50:
        parser.error("Require 1 <= --pairs <= --max-pairs <= 50")
    if not 0 <= args.cooldown <= 3600:
        parser.error("--cooldown must be within 0..3600 seconds")
    if args.probe_interval is not None and not 0.1 <= args.probe_interval <= 60:
        parser.error("--probe-interval must be within 0.1..60 seconds")
    if args.barrier_delay_ms is not None:
        if not 0 <= args.barrier_delay_ms <= 1000:
            parser.error("--barrier-delay-ms must be within 0..1000")
        for name, path in arms:
            if "barrier-delay" not in compiled_features(path):
                parser.error(f"Arm {name!r} is not a barrier-delay build")
    driver = args.driver[1:] if args.driver[:1] == ["--"] else args.driver
    if any(
        flag.startswith("--")
        and any(owned.startswith(flag.split("=", 1)[0]) for owned in ("--binary", "--output-dir"))
        for flag in driver
    ):
        parser.error("The session sets --binary and --output-dir for each run")
    args.driver = driver
    if args.output_dir.exists():
        parser.error("--output-dir must be new")
    return args


def compiled_features(binary):
    completed = run_process(
        [str(binary), "version", "--json"], check=False, capture_output=True, text=True, timeout=30
    )
    if completed.returncode != 0:
        raise RuntimeError(f"{binary} version --json exited {completed.returncode}")
    return json.loads(completed.stdout)["compiled_features"]


def sync(fd, full):
    # Ledger barriers use sync_all, which is F_FULLFSYNC on macOS; SQLite uses fsync.
    if full and sys.platform == "darwin":
        fcntl.fcntl(fd, fcntl.F_FULLFSYNC)
    else:
        os.fsync(fd)


def canary(directory):
    """Write throughput in MiB/s with ledger-style and SQLite-style syncs."""
    record = os.urandom(RECORD_BYTES)
    result = {}
    for kind, full in (("full_sync_mibs", True), ("plain_sync_mibs", False)):
        path = directory / f".canary-{kind}"
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        try:
            began = time.monotonic()
            for offset in range(0, CANARY_BYTES, RECORD_BYTES):
                check()
                view = memoryview(record)
                while view:
                    check()
                    view = view[os.write(fd, view) :]
                if (offset + RECORD_BYTES) % SYNC_BYTES == 0:
                    sync(fd, full)
            elapsed = time.monotonic() - began
        finally:
            os.close(fd)
            path.unlink()
        result[kind] = CANARY_BYTES / (1024 * 1024) / elapsed
    return result


class Probe:
    """Own the storage probe process, including during a blocked native flush."""

    def __init__(self, path, interval):
        self.path, self.interval = path, interval
        self.samples = []
        self.resources = contextlib.ExitStack()

    def __enter__(self):
        self.directory = self.path.with_name(self.path.name + "-" + uuid.uuid4().hex)
        create_directory(self.directory)
        try:
            self.child = self.resources.enter_context(
                spawn(
                    [
                        sys.executable,
                        "-m",
                        "e2e.scripts.storage_probe",
                        str(self.directory),
                        str(self.interval),
                    ],
                    cwd=ROOT,
                    stop_grace=2,
                )
            )
            if self.child.line(timeout=10) != b"ready\n":
                raise RuntimeError("Storage probe did not confirm readiness")
            return self
        except BaseException:
            with finalizer(self.resources.close):
                raise

    def __exit__(self, kind, error, traceback):
        with finalizer(self.finish):
            if error is not None:
                raise error.with_traceback(traceback)

    def finish(self):
        self.resources.close()
        if self.child.returncode != 128 + signal.SIGTERM:
            result = subprocess.CompletedProcess(
                self.child.command,
                self.child.returncode,
                bytes(self.child.output),
                bytes(self.child.errors),
            )
            raise ProcessFailure(result)
        self.samples = list(json_lines(self.directory / "samples.jsonl"))

    def metrics(self):
        values = [s["sync_ms"] for s in self.samples]
        if not values:
            return None
        slow = [s for s in self.samples if s["sync_ms"] > PROBE_SLOW_MS]
        return dict(
            samples=len(values),
            p50_ms=percentile(values, 0.5),
            p99_ms=percentile(values, 0.99),
            max_ms=max(values),
            over_30ms=sum(v > 30 for v in values),
            over_100ms=len(slow),
            first_over_100ms_s=(slow[0]["monotonic_ns"] - self.samples[0]["monotonic_ns"]) / 1e9
            if slow
            else None,
        )


def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(fraction * len(ordered)))] if ordered else None


def writer_metrics(path):
    """COMMIT and batch tails from an instrumented run's writer.batch records."""
    rows = []
    with path.open() as stream:
        for line in stream:
            row = json.loads(line)
            if row.get("event") == "writer.batch":
                rows.append(row)
    if not rows:
        return None
    commit = [r["commit_ns"] / 1e6 for r in rows]
    duration = [r["duration_ns"] / 1e6 for r in rows]
    # Builds before the deferred checkpoint record neither field; their idle
    # checkpoints count as "idle" and their WAL size is unknown.
    recorded = "frames" in rows[0]
    frames = [r["frames"] for r in rows] if recorded else []
    kinds = {}
    for r in rows:
        kind = r.get("checkpoint", "idle" if r["checkpoint_ns"] else "none")
        if kind != "none":
            kinds.setdefault(kind, []).append(r["checkpoint_ns"] / 1e6)
    return dict(
        batches=len(rows),
        commit_ms=dict(p50=percentile(commit, 0.5), p99=percentile(commit, 0.99), max=max(commit)),
        commits_over_100ms=sum(value > 100 for value in commit),
        batches_over_100ms=sum(value > 100 for value in duration),
        checkpoints={
            kind: dict(count=len(values), p99_ms=percentile(values, 0.99), max_ms=max(values))
            for kind, values in kinds.items()
        },
        peak_frames=max(frames) if recorded else None,
        # The WAL restarts when a commit's frame count falls below its predecessor's.
        restarts=sum(1 for before, after in zip(frames, frames[1:]) if after < before)
        if recorded
        else None,
    )


def logged_delay(directory):
    """The delay the daemon logged at startup, as the pair driver captured it."""
    startup = directory / "startup.json"
    return json.loads(startup.read_text())["barrier_delay_ms"] if startup.is_file() else None


def jsonl(path):
    with path.open() as stream:
        return [json.loads(line) for line in stream]


def arrival_metrics(directory):
    """Paced-arrival health : lateness and unfinished pairs."""
    schedule, progress = directory / "arrival-schedule.jsonl", directory / "progress.jsonl"
    if not schedule.is_file() or not progress.is_file():
        return None
    rows = [r for r in jsonl(schedule) if r["scheduled_monotonic_ns"] is not None]
    if not rows:
        return None
    epoch = rows[0]["scheduled_monotonic_ns"]
    last = max(r["finished_monotonic_ns"] for r in rows)
    lateness = [max(0, r["started_monotonic_ns"] - r["scheduled_monotonic_ns"]) / 1e9 for r in rows]
    during = [p for p in jsonl(progress) if epoch <= p["monotonic_ns"] <= last]
    unfinished = [(p["monotonic_ns"], p["submitted_pings"] - p["completed_pairs"]) for p in during]
    early = [count for at, count in unfinished if at - epoch <= 30e9]
    control = [
        p["control_probe_seconds"] for p in during if p.get("control_probe_seconds") is not None
    ]
    return dict(
        arrivals=len(rows),
        lateness_s=dict(p99=percentile(lateness, 0.99), max=max(lateness)),
        unfinished_first_30s_median=statistics.median(early) if early else None,
        unfinished_max=max((count for _, count in unfinished), default=None),
        control_p95_s=percentile(control, 0.95),
    )


def mode(canaries, reference, floor, probe=None):
    if probe and probe["over_100ms"]:
        return "slow"
    for sample in canaries:
        for kind, value in sample.items():
            if value < reference[kind] / 2 or (floor is not None and value < floor):
                return "slow"
    return "normal"


class Session:
    def __init__(self, args):
        self.args = args
        self.output = args.output_dir
        self.reference = None
        self.runs = []
        self.binaries = {}
        for name, path in args.arms:
            self.binaries[name] = sha256(path)

    def run_arm(self, pair, slot, name, binary):
        label = f"pair{pair:02d}-{slot}-{name}"
        with phase("wait"):
            pause(self.args.cooldown)
        before = canary(self.output)
        if self.reference is None:
            self.reference = before
        directory = self.output / label
        environment = dict(os.environ)
        environment.pop("HIVE_BARRIER_DELAY_MS", None)
        if self.args.barrier_delay_ms is not None:
            environment["HIVE_BARRIER_DELAY_MS"] = str(self.args.barrier_delay_ms)
        probe = (
            Probe(self.output / ".probe", self.args.probe_interval)
            if self.args.probe_interval
            else None
        )
        began = time.monotonic()
        with phase("scenario"), probe or contextlib.nullcontext():
            completed = run_scenario(
                [
                    sys.executable,
                    str(PAIR_DRIVER),
                    "mock",
                    "pairs",
                    "--binary",
                    str(binary),
                    "--output-dir",
                    str(directory),
                    *self.args.driver,
                ],
                check=False,
                stdout=subprocess.DEVNULL,
                env=environment,
                timeout=24 * 3600,
            )
        seconds = time.monotonic() - began
        if probe and directory.is_dir():
            atomic_write_text(directory / "probe.json", json.dumps(probe.samples) + "\n")
        after = canary(self.output)
        verification = directory / "verification.json"
        result = json.loads(verification.read_text()) if verification.is_file() else {}
        timings = directory / "debug-timings.jsonl"
        record = dict(
            label=label,
            pair=pair,
            slot=slot,
            arm=name,
            binary_sha256=self.binaries[name],
            returncode=completed.returncode,
            result=result.get("result", "missing") if completed.returncode == 0 else "failed",
            wall_seconds=seconds,
            canary_before=before,
            canary_after=after,
            mode=mode(
                (before, after),
                self.reference,
                self.args.canary_floor_mibs,
                probe.metrics() if probe else None,
            ),
            metrics={metric: result.get(metric) for metric in METRICS},
            writer=writer_metrics(timings) if timings.is_file() else None,
            arrivals=arrival_metrics(directory),
            probe=probe.metrics() if probe else None,
            barrier_delay_ms=logged_delay(directory),
        )
        self.runs.append(record)
        self.save()
        if (
            self.args.barrier_delay_ms is not None
            and completed.returncode == 0
            and record["barrier_delay_ms"] != self.args.barrier_delay_ms
        ):
            raise RuntimeError(f"{label} logged a barrier delay of {record['barrier_delay_ms']} ms")
        return record

    def same_mode_pairs(self):
        pairs = {}
        for run in self.runs:
            pairs.setdefault(run["pair"], []).append(run)
        return [
            runs
            for runs in pairs.values()
            if len(runs) == 2
            and runs[0]["mode"] == runs[1]["mode"]
            and all(run["result"] == "passed" and run["returncode"] == 0 for run in runs)
        ]

    def run(self):
        create_directory(self.output)
        (first, second) = self.args.arms
        for pair in range(self.args.max_pairs):
            order = (first, second) if pair % 2 == 0 else (second, first)
            for slot, (name, binary) in enumerate(order):
                self.run_arm(pair, slot, name, binary)
            if len(self.same_mode_pairs()) >= self.args.pairs:
                break
        self.save()
        print(self.output / "summary.md")

    def save(self):
        session = dict(
            protocol=dict(
                cooldown_seconds=self.args.cooldown,
                order="ABBA",
                probe_interval_seconds=self.args.probe_interval,
                barrier_delay_ms=self.args.barrier_delay_ms,
                canary_bytes=CANARY_BYTES,
                record_bytes=RECORD_BYTES,
                sync_bytes=SYNC_BYTES,
                slow_rule="either canary below half the reference or the floor, or a probe sync over 100 ms",
                floor_mibs=self.args.canary_floor_mibs,
                required_pairs=self.args.pairs,
                max_pairs=self.args.max_pairs,
                driver=self.args.driver,
            ),
            arms={
                name: dict(binary=str(path), sha256=self.binaries[name])
                for name, path in self.args.arms
            },
            reference=self.reference,
            runs=self.runs,
        )
        atomic_write_text(self.output / "session.json", json.dumps(session, indent=2) + "\n")
        atomic_write_text(self.output / "summary.md", self.summary())

    def summary(self):
        delay = self.args.barrier_delay_ms
        lines = [
            "# Arm session",
            "",
            f"Reference canary: {json.dumps(self.reference)}",
            "",
            *([f"Injected ledger barrier delay: {delay} ms", ""] if delay is not None else []),
            "| Arm | Mode | Runs | Passed | Median pairs/s | Median trace pairs/s |",
            "| --- | --- | --- | --- | --- | --- |",
        ]
        for name, _ in self.args.arms:
            for current in ("normal", "slow"):
                runs = [r for r in self.runs if r["arm"] == name and r["mode"] == current]
                passed = [r for r in runs if r["result"] == "passed" and r["returncode"] == 0]

                def median(metric):
                    values = [
                        r["metrics"][metric] for r in passed if r["metrics"][metric] is not None
                    ]
                    return f"{statistics.median(values):.3g}" if values else "-"

                lines.append(
                    f"| {name} | {current} | {len(runs)} | {len(passed)} | "
                    f"{median('pairs_per_second')} | {median('trace_pairs_per_second')} |"
                )
        lines += ["", "Same-mode pair differences (second arm minus first, trace pairs/s):", ""]
        (first, _), (second, _) = self.args.arms
        for runs in self.same_mode_pairs():
            by_arm = {r["arm"]: r["metrics"]["trace_pairs_per_second"] for r in runs}
            if None not in by_arm.values():
                lines.append(
                    f"- pair {runs[0]['pair']} ({runs[0]['mode']}): "
                    f"{by_arm[second] - by_arm[first]:+.3g}"
                )
        writers = [r for r in self.runs if r["writer"]]
        if writers:
            lines += [
                "",
                "| Run | Mode | COMMIT p50/p99/max ms | COMMIT >100 ms | Batches >100 ms | Checkpoints | Peak frames | Restarts |",
                "| --- | --- | --- | --- | --- | --- | --- | --- |",
            ]
            for r in writers:
                w = r["writer"]
                c = w["commit_ms"]
                checkpoints = (
                    ", ".join(
                        f"{k} {v['count']} (p99 {v['p99_ms']:.3g} ms)"
                        for k, v in w["checkpoints"].items()
                    )
                    or "none"
                )
                lines.append(
                    f"| {r['label']} | {r['mode']} | {c['p50']:.3g}/{c['p99']:.3g}/{c['max']:.3g} | "
                    f"{w['commits_over_100ms']} | {w['batches_over_100ms']} | {checkpoints} | {w['peak_frames']} | {w['restarts']} |"
                )
        probed = [r for r in self.runs if r["probe"]]
        if probed:
            lines += [
                "",
                "| Run | Mode | Pairs/s | Probe p50/p99/max ms | Probes >30 ms | >100 ms | First >100 ms s |",
                "| --- | --- | --- | --- | --- | --- | --- |",
            ]
            for r in probed:
                p = r["probe"]
                rate = r["metrics"]["pairs_per_second"]
                lines.append(
                    f"| {r['label']} | {r['mode']} | {rate if rate is None else f'{rate:.3g}'} | "
                    f"{p['p50_ms']:.3g}/{p['p99_ms']:.3g}/{p['max_ms']:.3g} | {p['over_30ms']} | "
                    f"{p['over_100ms']} | {p['first_over_100ms_s'] if p['first_over_100ms_s'] is None else round(p['first_over_100ms_s'])} |"
                )
        paced = [r for r in self.runs if r["arrivals"]]
        if paced:
            lines += [
                "",
                "| Run | Mode | Lateness p99/max s | Unfinished first 30 s median | Unfinished max | Control p95 s |",
                "| --- | --- | --- | --- | --- | --- |",
            ]
            for r in paced:
                a = r["arrivals"]
                control = f"{a['control_p95_s']:.3g}" if a["control_p95_s"] is not None else "-"
                lines.append(
                    f"| {r['label']} | {r['mode']} | {a['lateness_s']['p99']:.3g}/{a['lateness_s']['max']:.3g} | "
                    f"{a['unfinished_first_30s_median']} | {a['unfinished_max']} | {control} |"
                )
        mixed = [
            p
            for p in {r["pair"] for r in self.runs}
            if len({r["mode"] for r in self.runs if r["pair"] == p}) > 1
        ]
        lines += ["", f"Mixed-mode pairs (reported, not compared): {sorted(mixed) or 'none'}", ""]
        return "\n".join(lines)


def main():
    Session(parse_args()).run()


if __name__ == "__main__":
    raise SystemExit(entry(main, name="scenario"))
