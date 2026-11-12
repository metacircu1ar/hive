from contextlib import ExitStack
from release.owned_process import spawn
from release.files import create_directory

"""Ping pong pair test: N/2 top-level mock pings each spawn and shut down one pong.

The driver submits roots without waiting for pair completions. Runtime admission
is bounded by the mock high-residency limits; N is total identities, not residency.
"""
from release.cancellation import check, finish, pause
from release.process import run as run_process

#!/usr/bin/env python3
from release.files import atomic_copy, atomic_write_text

import argparse
import hashlib
import json
from release.files import raw_log, AppendStream, append_jsonl_record
from e2e.scripts.harness.io import CompleteLines
import math
import os
import platform
import resource
import shlex
import shutil
import sqlite3
import subprocess
import tempfile
import time
import uuid
from contextlib import closing
from dataclasses import dataclass
from pathlib import Path

from e2e.paths import product_default, product_root, require_paths

from e2e.scripts.harness.cli import Cli
from release.progress import cleanup_all, finalizer, phase
from e2e.scripts.run_mock_ping_pong_all_to_one import (
    PRESETS,
    mock_login,
    remaining_owned,
    require,
    runtime_limits,
)
from e2e.scripts.runtime_timings import cpu_snapshot, collect as collect_timings
from e2e.scripts.scenario_arrivals import submit as submit_arrival
from e2e.scripts.scenario_control import summarize as summarize_control
from e2e.scripts.scenario_control import validate_status
from e2e.scripts.scenario_host import HostObserver
from e2e.scripts.scenario_memory import Sampler
from e2e.scripts.scenario_observers import Observers
from e2e.scripts.scenario_progress import summarize as summarize_progress
from e2e.scripts.verify_ping_pong_pair import SEED, json_lines, verify


def admission_backlog(database):
    """One ready-index scan plus at most four indexed message/event lookups.

    Acceptance timestamps measure message age, not time since runnable. The
    caller owns a read snapshot; None denotes initialization without an inbox.
    """
    counts, oldest = {}, {}
    for kind, count, head in database.execute(
        "SELECT admission_class,count(*),min(pending_seq) FROM agents "
        "WHERE runnable=1 GROUP BY admission_class"
    ):
        counts[kind] = count
        if head is None:
            oldest[kind] = None
            continue
        message, recipient, startup = database.execute(
            "SELECT id,recipient,startup FROM messages WHERE seq=?", (head,)
        ).fetchone()
        event = database.execute(
            "SELECT at FROM events WHERE agent=? AND kind=? "
            "AND json_extract(data,?)=? ORDER BY seq LIMIT 1",
            (
                recipient,
                "spawned" if startup else "message_accepted",
                "$.initial_message_id" if startup else "$.id",
                message,
            ),
        ).fetchone()
        require(event is not None, "Pending message lacks acceptance evidence")
        oldest[kind] = event[0]
    return counts, oldest


def parse_args(argv=None):
    project = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--agents", type=int, choices=(100, 1000, 10000, 100000, 1000000), default=100
    )
    parser.add_argument("--binary", type=Path, default=product_default("target/release/hivectl"))
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--preset", choices=PRESETS, default="high-residency")
    parser.add_argument(
        "--process-variants",
        type=int,
        choices=(1, 2, 3),
        default=1,
        help="Split roots across default/true/false Codex compression settings",
    )
    parser.add_argument(
        "--logins",
        type=int,
        choices=(1, 2, 3),
        default=1,
        help="Mock subscription logins; more than one selects them through a group",
    )
    parser.add_argument(
        "--auth-mode",
        choices=("ordered", "balanced"),
        default="balanced",
        help="Group mode used when --logins is above one",
    )
    parser.add_argument(
        "--deltas",
        type=int,
        default=0,
        help="Streamed text deltas per turn, sent only when the client did not opt out",
    )
    for flag in ("max-loaded", "max-providers", "max-active-turns", "max-starting"):
        parser.add_argument("--" + flag, type=int)
    parser.add_argument("--warm-providers", type=int, default=0)
    parser.add_argument("--idle-seconds", type=int, default=0)
    parser.add_argument("--linger-seconds", type=int, default=2)
    parser.add_argument(
        "--cohort-min-seconds",
        type=int,
        help="Minimum provider cohort residency; the product default when omitted",
    )
    parser.add_argument(
        "--debug-timings",
        action="store_true",
        help="Collect bounded DEBUG tracing intervals; do not use timings as uninstrumented benchmarks",
    )
    parser.add_argument(
        "--root-rate",
        type=float,
        default=0,
        help="Paced roots per second; 0 submits bursts of 64. Late arrivals are recorded, never skipped.",
    )
    parser.add_argument(
        "--observer-interval",
        type=float,
        default=0,
        help="Independent control/progress sampling interval; 0 disables",
    )
    parser.add_argument("--timeout", type=int, default=28800)
    args = parser.parse_args(argv)
    require_paths(args, parser, "binary")
    if not math.isfinite(args.root_rate) or (
        args.root_rate != 0 and not 0.01 <= args.root_rate <= 1000
    ):
        parser.error("--root-rate must be 0 (burst) or between 0.01 and 1000")
    if not 0 <= args.idle_seconds <= 86400:
        parser.error("--idle-seconds must be between 0 and 86400")
    if not 0 <= args.linger_seconds <= 3600:
        parser.error("--linger-seconds must be between 0 and 3600")
    if args.cohort_min_seconds is not None and not 0 <= args.cohort_min_seconds <= 3600:
        parser.error("--cohort-min-seconds must be between 0 and 3600")
    if args.observer_interval != 0 and not 0.05 <= args.observer_interval <= 10:
        parser.error("--observer-interval must be 0 or between 0.05 and 10 seconds")
    if not 0 <= args.deltas <= 1000:
        parser.error("--deltas must be between 0 and 1000")
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    try:
        runtime_limits(args)
    except RuntimeError as error:
        parser.error(str(error))
    args.output_dir = (
        args.output_dir
        or product_root() / "dist" / "e2e" / f"mock-ping-pong-pair-{args.agents}-{uuid.uuid4()}"
    )
    return args


def run(args):
    scenario = PairRun(args).run()
    check()
    return scenario.report()


class PairRun:
    """Own one scenario and retain its evidence through every cleanup path."""

    def __init__(self, args):
        self.args = args
        self.began = time.monotonic()
        self.harness_cpu = {"start": cpu_snapshot()} if self.args.debug_timings else None
        self.project = Path(__file__).resolve().parent.parent
        self.binary = self.args.binary.resolve()
        require(
            self.binary.is_file() and self.binary.parent.name == "release",
            "Build first; binary must be a release build",
        )
        self.limits = runtime_limits(self.args)
        self.limits["idle_seconds"] = self.args.idle_seconds
        self.limits["linger_seconds"] = self.args.linger_seconds
        if self.args.cohort_min_seconds is not None:
            self.limits["cohort_min_seconds"] = self.args.cohort_min_seconds
        if self.args.process_variants > 1:
            self.limits["max_process_variants"] = self.args.process_variants
        self.output = self.args.output_dir.resolve()
        self.prepare_output()
        self.prepare_config()
        self.save_build()
        self.save_settings()
        self.command = [
            str(self.binary),
            "--config",
            str(self.config),
            "--state-dir",
            str(self.state),
            "--codex-bin",
            str(self.provider),
        ]

        remote = [self.command[0], "--state-dir", str(self.state)]
        control = remote + ["--config", str(self.config)]
        self.call = Cli(remote, prefixes={"daemon": control}, timeout=60)

        self.daemon = self.sampler = self.database = self.observers = None
        self.success = False
        self.retained_state = None
        self.timings = {}
        self.host = HostObserver(self.output)

    def prepare_output(self):
        required_disk = self.args.agents * 30000 + 8 * 1024**3
        ancestor = self.output.parent
        while not ancestor.exists():
            ancestor = ancestor.parent
        free_disk = shutil.disk_usage(ancestor).free
        print(
            json.dumps(
                dict(
                    phase="disk_preflight",
                    free_bytes=free_disk,
                    estimated_required_bytes=required_disk,
                )
            ),
            flush=True,
        )
        require(
            free_disk >= required_disk,
            "Insufficient disk for estimated evidence, scratch and reserve",
        )
        create_directory(self.output)
        self.args.output_owned = True
        device = self.output.stat().st_dev
        candidates = (Path("/tmp"), Path(tempfile.gettempdir()))
        scratch_parent = next(
            (candidate for candidate in candidates
             if candidate.is_dir() and candidate.stat().st_dev == device),
            self.output.parent,
        )
        self.temporary = Path(tempfile.mkdtemp(prefix="hl-pair-", dir=scratch_parent))
        require(
            self.temporary.stat().st_dev == self.output.stat().st_dev,
            "Output must share the temporary-state filesystem",
        )
        self.state = self.temporary / "state"
        self.config = self.temporary / "config.json"
        self.provider = self.temporary / "mock-provider"
        atomic_copy(self.project / "tests/fixtures/scale_provider.py", self.provider, mode=0o700)
        if self.args.deltas:
            atomic_write_text(
                self.temporary / "scale-scenario.json", json.dumps({"deltas": self.args.deltas})
            )
        atomic_copy(
            product_root() / "tests/fixtures/mock_common.py", self.temporary / "mock_common.py"
        )
        atomic_copy(
            product_root() / "tests/fixtures/codex-schema.json",
            self.temporary / "codex-schema.json",
        )

    def logins(self):
        return ["mock"] + [f"mock-{index}" for index in range(2, self.args.logins + 1)]

    def prepare_config(self):
        configuration = dict(
            auth_profiles={
                name: dict(provider="codex", method="subscription") for name in self.logins()
            },
            runtime=self.limits,
            roles={},
        )
        auth = "mock"
        if self.args.logins > 1:
            auth = "mock-pool"
            configuration["auth_groups"] = {
                auth: dict(mode=self.args.auth_mode, members=self.logins())
            }
        for role, lifetime in [
            ("pair-ping", "independent"),
            ("pair-pong", "independent"),
        ]:
            instructions = self.temporary / f"{role}.md"
            atomic_write_text(instructions, "Deterministic mock ping pong pair role.\n")
            configuration["roles"][role] = dict(
                auth=auth,
                model="gpt-5.6-luna",
                effort="low",
                instructions=str(instructions),
                lifetime=lifetime,
                sandbox="read-only",
            )
        atomic_write_text(self.config, json.dumps(configuration, indent=2) + "\n")
        atomic_copy(self.config, self.output / "config.json")
        atomic_copy(self.provider, self.output / "mock-provider.py")
        atomic_copy(self.temporary / "mock_common.py", self.output / "mock_common.py")
        atomic_copy(self.temporary / "codex-schema.json", self.output / "codex-schema.json")
        atomic_copy(product_root() / "scripts/release/files.py", self.temporary / "mock_files.py")
        atomic_copy(self.temporary / "mock_files.py", self.output / "mock_files.py")

    def save_build(self):
        atomic_write_text(
            self.output / "build.json",
            json.dumps(
                dict(
                    binary=str(self.binary),
                    profile="release",
                    binary_sha256=hashlib.sha256(self.binary.read_bytes()).hexdigest(),
                    fixture_sha256=hashlib.sha256(self.provider.read_bytes()).hexdigest(),
                    host=platform.platform(),
                ),
                indent=2,
            )
            + "\n",
        )

    def save_settings(self):
        atomic_write_text(
            self.output / "run-settings.json",
            json.dumps(
                dict(
                    debug_timings=self.args.debug_timings,
                    idle_seconds=self.args.idle_seconds,
                    linger_seconds=self.args.linger_seconds,
                    cohort_min_seconds=self.args.cohort_min_seconds,
                    process_variants=self.args.process_variants,
                    logins=self.args.logins,
                    auth_mode=self.args.auth_mode if self.args.logins > 1 else None,
                    deltas=self.args.deltas,
                    root_rate=self.args.root_rate,
                    root_schedule="Fixed monotonic deadlines; sequential acknowledgements; no dropped or retried arrivals.",
                    observer_interval=self.args.observer_interval,
                    progress_poll_seconds=1,
                    backlog_sample_seconds=10,
                    memory_sample_seconds=1,
                    submission_progress_throttle_seconds=2,
                    host_observation_seconds=60,
                    control_probe_note="One request per monitor; cadence slows with observer latency.",
                ),
                indent=2,
            )
            + "\n",
        )

    def run(self):
        with ExitStack() as resources:
            self.resources = resources
            with finalizer(self.cleanup):
                with phase("daemon"):
                    self.start()
                self.monitor = PairMonitor(self)
                self.submit()
                with phase("wait"):
                    self.wait_for_completion()
                with phase("shutdown"):
                    self.shutdown()
                self.verify_control()
                self.verify_evidence()
                self.collect_timings()
                self.measure()
                self.retain_evidence()
        self.prepare_report()
        return self

    def start(self):
        before = time.monotonic()
        mock_login(self.command)
        for name in self.logins()[1:]:
            # `script` supplies the terminal that native login expects. BSD script
            # takes the command as arguments and returns its status; util-linux
            # needs -c and -e for that status.
            login = [*self.command, "auth", "login", name]
            terminal = (
                ["script", "-q", "/dev/null", *login]
                if platform.system() == "Darwin"
                else ["script", "-q", "-e", "-c", shlex.join(login), "/dev/null"]
            )
            run_process(
                terminal,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=60,
                check=True,
            )
        self.timings["mock_login_seconds"] = time.monotonic() - before
        before = time.monotonic()
        daemon_env = os.environ.copy()
        daemon_env.pop("HIVE_DIAGNOSTICS", None)
        daemon_env.pop("HIVE_PROFILE", None)
        daemon_env["HIVE_LOG"] = "debug" if self.args.debug_timings else "info"
        log = self.resources.enter_context(raw_log(self.output / "daemon.log"))
        self.daemon = self.resources.enter_context(
            spawn(
                self.command + ["daemon", "run"],
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=log,
                env=daemon_env,
                limit=64 * 1024 * 1024,
            )
        )
        for _ in range(200):
            require(not self.daemon.exited(), "Daemon exited during startup")
            try:
                status = self.call("daemon", "status")
                break
            except (RuntimeError, json.JSONDecodeError):
                pause(0.05)
        else:
            raise RuntimeError("Daemon startup timed out")
        self.ready_ns = time.monotonic_ns()
        self.timings["daemon_start_seconds"] = time.monotonic() - before
        if "barrier-delay" in (status.get("compiled_features") or []):
            self.record_barrier_delay()
        self.sampler = Sampler(
            status["pid"], self.output, max_rss_gib=8, cpu=self.args.debug_timings
        )
        self.sampler.start()
        if self.args.observer_interval:
            self.observers = Observers(
                self.state,
                self.output,
                self.daemon.pid,
                self.args.observer_interval,
            )
            self.observers.start()
        self.database = sqlite3.connect(
            f"file:{self.state}/runtime/runtime.sqlite?mode=ro", uri=True
        )
        self.database.execute("PRAGMA cache_size=-4096")
        atomic_write_text(
            self.output / "checkpoint.json",
            json.dumps(dict(command=self.command, state_dir=str(self.state)), indent=2) + "\n",
        )

    def record_barrier_delay(self):
        """A measurement build logs its delay once at startup.
        Debug runs rotate that segment away, so read it now into startup.json."""
        path = self.state / "runtime/logs/hive-daemon.jsonl"
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if path.is_file():
                with CompleteLines(path) as lines:
                    for line in lines:
                        row = json.loads(line)
                        if row.get("event") == "state.barrier_delay":
                            atomic_write_text(
                                self.output / "startup.json",
                                json.dumps(dict(barrier_delay_ms=row["duration_ms"])) + "\n",
                            )
                            return
            pause(0.1)
        raise RuntimeError("The barrier-delay build logged no delay at startup")

    def submit(self):
        self.submission_start_ns = time.monotonic_ns()
        batch_file = self.temporary / "batch.json"
        batch_size = 1 if self.args.root_rate else 64
        with (
            AppendStream.create_new(self.output / "submissions.jsonl", durable=False) as seeds,
            AppendStream.create_new(
                self.output / "arrival-schedule.jsonl", durable=False
            ) as arrivals,
        ):
            for offset in range(0, self.args.agents // 2, batch_size):
                operations = [
                    dict(
                        op="spawn",
                        role="pair-ping",
                        cwd=str(self.temporary),
                        body=SEED,
                        request_id=f"pair-{i}",
                        **(
                            {
                                "options": {
                                    "codex.request_compression": i % self.args.process_variants == 1
                                }
                            }
                            if i % self.args.process_variants
                            else {}
                        ),
                    )
                    for i in range(offset, min(offset + batch_size, self.args.agents // 2))
                ]
                atomic_write_text(batch_file, json.dumps(operations))
                results = submit_arrival(
                    arrivals,
                    self.submission_start_ns,
                    self.args.root_rate,
                    offset,
                    len(operations),
                    lambda: self.call(
                        "batch",
                        "--operations-file",
                        str(batch_file),
                        "--request-id",
                        f"seed-batch-{offset}",
                    )["results"],
                    self.monitor.check_running,
                    self.monitor,
                )
                for self.result in results:
                    seeds.append_jsonl(self.result)
                self.monitor.submitted += len(results)
                self.monitor()
        self.submission_end_ns = time.monotonic_ns()
        self.timings["root_submission_seconds"] = (
            self.submission_end_ns - self.submission_start_ns
        ) / 1e9

    def wait_for_completion(self):
        while True:
            active = self.monitor(force=True)
            if active == 0:
                require(
                    self.monitor.completed == self.monitor.cancelled == self.args.agents // 2,
                    "Roots stopped before every pair completed",
                )
                break
            pause(1)
        self.complete_ns = time.monotonic_ns()
        self.timings["execution_after_submission_seconds"] = (
            self.complete_ns - self.submission_end_ns
        ) / 1e9
        self.timings["scenario_seconds"] = time.monotonic() - self.began

    def shutdown(self):
        before = time.monotonic()
        if self.observers is not None:
            self.observers.stop()
        self.sampler.stop()
        self.sampler.check()
        require(not self.sampler.failure, str(self.sampler.failure))
        self.database.close()
        self.database = None
        self.call("daemon", "stop")
        require(self.daemon.wait(timeout=60) == 0, "Daemon cleanup failed")
        wal = self.state / "runtime/runtime.sqlite-wal"
        require(
            not wal.exists() or wal.stat().st_size == 0,
            "Runtime WAL was not checkpointed",
        )
        require(not remaining_owned(self.sampler), "Owned mock process survived shutdown")
        self.timings["shutdown_seconds"] = time.monotonic() - before
        if self.harness_cpu is not None:
            self.harness_cpu["after_shutdown"] = cpu_snapshot()

    def verify_control(self):
        self.control_error = None
        self.control_failure = None
        control_started = time.monotonic()
        try:
            control_summary = summarize_control(
                self.output / "control.jsonl",
                self.daemon.pid,
            )
            atomic_write_text(
                self.output / "control-summary.json", json.dumps(control_summary, indent=2) + "\n"
            )
            if self.observers is not None:
                self.observers.summarize()
                progress_summary = summarize_progress(
                    self.output / "fast-progress.jsonl", self.output / "fast-control.jsonl"
                )
                require(
                    progress_summary["progress_samples"]
                    == self.observers.reports["progress"]["samples"],
                    "Progress observer sample count disagrees",
                )
                atomic_write_text(
                    self.output / "fast-progress-summary.json",
                    json.dumps(progress_summary, indent=2) + "\n",
                )
        except (RuntimeError, OSError, KeyError, TypeError, ValueError) as error:
            self.control_failure = error
            self.control_error = str(error)
            atomic_write_text(
                self.output / "control-error.json",
                json.dumps(dict(error=self.control_error)) + "\n",
            )
        self.timings["control_verification_seconds"] = time.monotonic() - control_started

    def verify_evidence(self):
        self.traces = sorted(self.state.glob("profiles/*/provider/scale-mock/trace-*.jsonl"))
        require(self.traces, "Missing provider traces")
        print(
            json.dumps(
                dict(phase="verification", scenario_seconds=self.timings["scenario_seconds"])
            ),
            flush=True,
        )
        require(
            shutil.disk_usage(self.output).free >= self.args.agents * 4000 + 4 * 1024**3,
            "Insufficient disk for verifier scratch and reserve",
        )
        if self.harness_cpu is not None:
            self.harness_cpu["before_audit"] = cpu_snapshot()
        self.result = verify(
            self.state / "runtime/runtime.sqlite",
            self.output / "submissions.jsonl",
            self.traces,
            self.args.agents,
            self.limits,
            self.output / "audit.sqlite",
        )
        if self.args.process_variants > 1:
            self.verify_process_options()
        if self.args.logins > 1:
            self.verify_logins()

    def verify_logins(self):
        """Every agent is on a pool member; ordered uses only the first, balanced all."""
        with closing(
            sqlite3.connect(f"file:{self.state}/runtime/runtime.sqlite?mode=ro", uri=True)
        ) as database:
            rows = database.execute(
                "SELECT auth, json_extract(data,'$.auth_decision.reason'), count(*) "
                "FROM agents GROUP BY 1, 2 ORDER BY 1, 2"
            ).fetchall()
            loaded = database.execute("SELECT count(*) FROM auth_load").fetchone()[0]
        per_login = {name: 0 for name in self.logins()}
        reasons = {}
        for auth, reason, count in rows:
            require(auth in per_login, "Agent bound outside the login pool")
            per_login[auth] += count
            reasons[reason] = reasons.get(reason, 0) + count
        require(sum(per_login.values()) == self.args.agents, "Unexpected pooled agent count")
        require(loaded == 0, "Ended agents retained bound-login load")
        if self.args.auth_mode == "ordered":
            require(
                per_login["mock"] == self.args.agents and set(reasons) == {"ordered"},
                "Ordered pool left its first usable member",
            )
        else:
            fair = self.args.agents / self.args.logins
            require(set(reasons) == {"balanced"}, "Unexpected balanced decision reason")
            require(min(per_login.values()) >= fair * 0.8, "Balanced pool starved a member")
        self.result["login_audit"] = dict(
            mode=self.args.auth_mode, agents_per_login=per_login, decision_reasons=reasons
        )

    def verify_process_options(self):
        with closing(
            sqlite3.connect(f"file:{self.state}/runtime/runtime.sqlite?mode=ro", uri=True)
        ) as database:
            processes, sessions = {}, 0
            for path in self.traces:
                selected = None
                for row in json_lines(path):
                    if row["kind"] == "process_start":
                        selected = row["compression"]
                        processes[selected] = processes.get(selected, 0) + 1
                    elif row["kind"] in ("thread_start", "thread_resume"):
                        saved = database.execute(
                            "SELECT json_extract(data,'$.agent_options.baseline.values.\"codex.request_compression\"') FROM agents WHERE id=?",
                            (row["agent"],),
                        ).fetchone()
                        require(saved is not None, "Missing variant agent")
                        expected = (
                            "default" if saved[0] is None else "true" if saved[0] == 1 else "false"
                        )
                        require(
                            selected == expected,
                            "Conversation acquired a mismatched process variant",
                        )
                        sessions += 1
            expected_keys = {"default", "true"} | (
                {"false"} if self.args.process_variants == 3 else set()
            )
            require(
                set(processes) == expected_keys and sessions >= self.args.agents,
                "Incomplete variant coverage",
            )
            require(
                database.execute("SELECT count(*) FROM process_variants").fetchone()[0] == 0,
                "Ended agents retained logical process-variant claims",
            )
            self.result["process_variant_audit"] = dict(
                process_counts=processes, checked_sessions=sessions
            )

    def collect_timings(self):
        self.timings_error = None
        self.timings_failure = None
        if self.harness_cpu is not None:
            self.harness_cpu["after_audit"] = cpu_snapshot()
            try:
                collect_timings(self.state, self.output, self.daemon.pid)
            except (RuntimeError, OSError, KeyError, TypeError, ValueError) as error:
                self.timings_error = str(error)
                self.timings_failure = error
                atomic_write_text(
                    self.output / "debug-timings-error.json",
                    json.dumps(dict(error=self.timings_error)) + "\n",
                )

    def measure(self):
        self.trace = self.result.pop("trace_times_ns")
        self.timings.update(self.result.pop("verification_seconds"))
        self.timings.update(
            {
                name + "_after_ready_seconds": (self.trace[key] - self.ready_ns) / 1e9
                for name, key in [
                    ("first_provider", "first_start"),
                    ("last_turn", "last_turn"),
                    ("last_provider_exit", "last_exit"),
                ]
            }
        )
        self.timings["observed_completion_after_ready_seconds"] = (
            self.complete_ns - self.ready_ns
        ) / 1e9
        self.peak_total = self.peak_daemon = self.samples = 0
        for sample in json_lines(self.output / "memory.jsonl"):
            self.samples += 1
            self.peak_total = max(self.peak_total, sample["total_rss_kib"])
            self.peak_daemon = max(self.peak_daemon, sample["daemon_rss_kib"])
        self.sizes = dict(
            runtime_database_bytes=(self.state / "runtime/runtime.sqlite").stat().st_size,
            trace_bytes=sum(p.stat().st_size for p in self.traces),
            audit_bytes=(self.output / "audit.sqlite").stat().st_size,
            fixture_database_bytes=sum(
                p.stat().st_size
                for p in self.state.glob("profiles/*/provider/scale-mock/pairs.sqlite*")
            ),
        )

    def retain_evidence(self):
        # Move, do not double the multi-GB evidence archive with a database backup.
        # Require the same filesystem; never silently copy a million-agent state.
        require(
            self.state.stat().st_dev == self.output.stat().st_dev,
            "Output must share the temporary-state filesystem",
        )
        self.state.rename(self.output / "state")
        self.retained_state = self.output / "state"
        (self.output / "audit.sqlite").unlink()  # Reproducible derivative, not original evidence.
        before = time.monotonic()
        shutil.rmtree(self.temporary)
        self.timings["temporary_cleanup_seconds"] = time.monotonic() - before
        self.timings["total_harness_seconds"] = time.monotonic() - self.began
        self.peak_self = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
        if self.harness_cpu is not None:
            self.harness_cpu["end"] = cpu_snapshot()
            self.harness_cpu["metric"] = (
                "RUSAGE_SELF includes harness threads, excludes observer/profiler child CPU; brackets are cumulative"
            )
            atomic_write_text(
                self.output / "harness-cpu.json", json.dumps(self.harness_cpu, indent=2) + "\n"
            )

    def prepare_report(self):
        failures = (
            (self.control_error, "control"),
            (self.timings_error, "debug_timings"),
        )
        self.result.update(
            result="failed" if any(error is not None for error, _ in failures) else "passed",
            scenario="ping pong pair",
            preset=self.args.preset,
            process_variants=self.args.process_variants,
            logins=self.args.logins,
            auth_mode=self.args.auth_mode if self.args.logins > 1 else None,
            deltas=self.args.deltas,
            root_rate=self.args.root_rate,
            build_profile="release",
            limits=self.limits,
            timings=self.timings,
            debug_timings_enabled=self.args.debug_timings,
            debug_timings_error=self.timings_error,
            control_error=self.control_error,
            backlog_scan_seconds=self.monitor.backlog_scan_seconds,
            host_observation_seconds=self.host.seconds,
            messages_per_second=self.result["messages"] / self.timings["scenario_seconds"],
            trace_messages_per_second=self.result["messages"]
            / ((self.trace["last_turn"] - self.trace["first_start"]) / 1e9),
            trace_pairs_per_second=self.result["pairs"]
            / ((self.trace["last_turn"] - self.trace["first_start"]) / 1e9),
            pairs_per_second=self.result["pairs"] / self.timings["scenario_seconds"],
            sampled_peak_rss_mib=self.peak_total / 1024,
            sampled_peak_daemon_rss_mib=self.peak_daemon / 1024,
            harness_peak_rss_mib=self.peak_self
            / (1024**2 if platform.system() == "Darwin" else 1024),
            memory_samples=self.samples,
            storage=self.sizes,
            memory_metric="Daemon and observed provider RSS sampled each second; shared pages included, short peaks may be missed. Harness peak reported separately.",
            latency_clock="Persisted event timestamps have whole-second resolution; timings use monotonic clocks.",
        )
        for error, kind in failures:
            if error is not None:
                atomic_write_text(
                    self.output / "verification.json", json.dumps(self.result, indent=2) + "\n"
                )
                cause = getattr(
                    self, "control_failure" if kind == "control" else "timings_failure", None
                )
                raise RuntimeError(
                    f"Pair audit passed, but {kind} comparison failed: {error}"
                ) from cause
        (self.output / "failure-state.txt").unlink(missing_ok=True)

    def report(self):
        atomic_write_text(
            self.output / "verification.json", json.dumps(self.result, indent=2) + "\n"
        )
        print(json.dumps(self.result), flush=True)
        self.success = True
        return self.result

    def cleanup(self):
        with finalizer(self.retain_failure):
            try:
                cleanup_all(
                    self.stop_observers,
                    self.stop_sampler,
                    self.stop_daemon,
                    self.close_database,
                    self.close_owner,
                )
            except BaseException:
                self.success = False
                raise

    def stop_observers(self):
        if self.observers is not None:
            self.observers.stop()

    def stop_sampler(self):
        if self.sampler:
            self.sampler.stop()

    def close_owner(self):
        resources = getattr(self, "resources", None)
        if resources is not None:
            resources.close()

    def kill_daemon(self):
        if self.daemon is not None:
            self.daemon.force()

    def stop_daemon(self):
        if self.daemon is not None and not self.daemon.exited():
            try:
                self.call("daemon", "stop")
                self.daemon.wait(timeout=60)
            except Exception as error:

                def record():
                    atomic_write_text(
                        self.output / "cleanup-error.json",
                        json.dumps(
                            dict(
                                error=str(error),
                                remaining_owned=remaining_owned(self.sampler)
                                if self.sampler
                                else [],
                            ),
                            indent=2,
                        )
                        + "\n",
                    )

                with finalizer(record), finalizer(self.kill_daemon):
                    raise

    def close_database(self):
        if self.database:
            self.database.close()

    def retain_failure(self):
        if not self.success:
            retained = self.retained_state or self.state
            if (
                self.state.exists()
                and self.daemon is not None
                and self.daemon.exited()
                and not (remaining_owned(self.sampler) if self.sampler else [])
            ):
                self.state.rename(self.output / "state")
                retained = self.output / "state"
            atomic_write_text(self.output / "failure-state.txt", str(retained) + "\n")


@dataclass
class ProgressSnapshot:
    snapshot_before_ns: int
    snapshot_wall_ns: int
    latest: int
    snapshot_after_ns: int
    counts: dict[str, int]
    active: int


class PairMonitor:
    """Admission and progress counters for one sequential observation stream."""

    def __init__(self, run):
        self.run = run
        self.submitted = self.completed = self.cancelled = 0
        self.seq = 0
        self.last_progress = time.monotonic()
        self.last_monitor = 0
        self.last_backlog = 0
        self.last_backlog_ns = 0
        self.last_backlog_end_ns = 0
        self.last_backlog_wall_ns = 0
        self.backlog_scan_seconds = 0.0
        self.backlog = {}
        self.backlog_oldest = {}
        self.deadline = time.monotonic() + self.run.args.timeout
        self.control_index = 0

    def control_status(self, active):
        kind = "cli"
        self.control_index += 1
        context = dict(
            probe_index=self.control_index - 1,
            submitted_pings=self.submitted,
            completed_pairs=self.completed,
            nonterminal_or_draining=active,
            monotonic_implementation=time.get_clock_info("monotonic").implementation,
        )
        observation = dict(kind=kind, start_monotonic_ns=time.monotonic_ns())

        def record():
            observation.update(context)
            observation["end_monotonic_ns"] = time.monotonic_ns()
            observation["seconds"] = (
                observation["end_monotonic_ns"] - observation["start_monotonic_ns"]
            ) / 1e9
            append_jsonl_record(self.run.output / "control.jsonl", observation, durable=False)

        with finalizer(record):
            try:
                status = self.run.call("daemon", "status")
                validate_status(status, self.run.daemon.pid)
                observation["status"] = status
                return status, observation
            except Exception as error:
                observation["error"] = f"{type(error).__name__}: {error}"
                raise

    def check_running(self):
        now = time.monotonic()
        require(not self.run.daemon.exited(), "Daemon exited during scenario")
        self.run.sampler.check()
        require(not self.run.sampler.failure, str(self.run.sampler.failure))
        require(now < self.deadline, "Scenario deadline exceeded")
        return now

    def __call__(self, force=False):
        now = self.check_running()
        if not force and now - self.last_monitor < 2:
            return None
        self.run.host.sample()
        require(
            shutil.disk_usage(self.run.output).free > 8 * 1024**3,
            "Less than 8 GiB free disk; stopping with evidence retained",
        )
        snapshot = self.snapshot(now)
        require(
            not snapshot.counts.get("execution_failed"),
            "Agent failed; inspect retained state",
        )
        self.completed += snapshot.counts.get("task_completed", 0)
        self.cancelled += snapshot.counts.get("termination_requested", 0)
        if snapshot.latest != self.seq:
            self.last_progress = now
        self.seq = snapshot.latest
        require(now - self.last_progress < 120, "No durable progress for 120 seconds")
        status, control = self.control_status(snapshot.active)
        progress = dict(
            elapsed_seconds=round(now - self.run.began, 3),
            monotonic_ns=time.monotonic_ns(),
            wall_ns=time.time_ns(),
            monotonic_implementation=time.get_clock_info("monotonic").implementation,
            control_probe_kind=control["kind"],
            control_probe_seconds=control["seconds"],
            backlog_sample_monotonic_ns=self.last_backlog_ns,
            backlog_sample_end_monotonic_ns=self.last_backlog_end_ns,
            backlog_sample_wall_ns=self.last_backlog_wall_ns,
            submitted_pings=self.submitted,
            completed_pairs=self.completed,
            cancelled_pongs=self.cancelled,
            nonterminal_or_draining=snapshot.active,
            event_seq=self.seq,
            event_snapshot_before_monotonic_ns=snapshot.snapshot_before_ns,
            event_snapshot_after_monotonic_ns=snapshot.snapshot_after_ns,
            event_counts=snapshot.counts,
            runnable_by_admission_class=self.backlog,
            oldest_pending_accepted_at_by_admission_class=self.backlog_oldest,
            backlog_scan_seconds=self.backlog_scan_seconds,
            backlog_sample_elapsed_seconds=self.last_backlog - self.run.began,
            storage=status.get("storage"),
            daemon_rss_mib=round(self.run.sampler.latest["daemon_rss_kib"] / 1024, 2),
            pool=status.get("runtime_resources"),
            resident_actors=status.get("resident_actors"),
            database_bytes=(self.run.state / "runtime/runtime.sqlite").stat().st_size,
            wal_bytes=(
                (self.run.state / "runtime/runtime.sqlite-wal").stat().st_size
                if (self.run.state / "runtime/runtime.sqlite-wal").exists()
                else 0
            ),
        )
        append_jsonl_record(self.run.output / "progress.jsonl", progress, durable=False)
        print(json.dumps(progress), flush=True)
        self.last_monitor = now
        return snapshot.active

    def snapshot(self, now):
        self.run.database.execute("BEGIN")
        with finalizer(self.run.database.rollback):
            snapshot_before_ns = time.monotonic_ns()
            snapshot_wall_ns = time.time_ns()
            latest = self.run.database.execute(
                "SELECT coalesce(max(seq),0) FROM events"
            ).fetchone()[0]
            snapshot_after_ns = time.monotonic_ns()
            counts = dict(
                self.run.database.execute(
                    "SELECT kind,count(*) FROM events WHERE seq>? AND seq<=? GROUP BY kind",
                    (self.seq, latest),
                )
            )
            if now - self.last_backlog >= 10:
                scan_started = time.monotonic()
                self.last_backlog_ns = snapshot_before_ns
                self.last_backlog_end_ns = snapshot_after_ns
                self.last_backlog_wall_ns = snapshot_wall_ns
                self.backlog, self.backlog_oldest = admission_backlog(self.run.database)
                self.backlog_scan_seconds += time.monotonic() - scan_started
                self.last_backlog = now
            active = self.run.database.execute("SELECT active FROM runtime_counts").fetchone()[0]
        return ProgressSnapshot(
            snapshot_before_ns, snapshot_wall_ns, latest, snapshot_after_ns, counts, active
        )


def main():
    args = parse_args()
    try:
        scenario = PairRun(args).run()
        finish(scenario.report)
    except BaseException as error:
        if getattr(args, "output_owned", False):
            with finalizer(
                lambda: atomic_write_text(
                    args.output_dir / "failure.json",
                    json.dumps(dict(result="failed", error=str(error)), indent=2) + "\n",
                )
            ):
                raise
        raise


if __name__ == "__main__":
    from release.progress import entry

    raise SystemExit(entry(main, name="scenario"))
