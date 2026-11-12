"""Optional independent, streaming control and committed-progress observers."""

from release.files import atomic_write_text
from release.progress import event, finalizer
from e2e.scripts.harness.worker import Worker

import json
from release.files import AppendStream
import math
import sqlite3
import threading
import time
from contextlib import closing
from pathlib import Path

from e2e.scripts.scenario_control import ping, summarize


class Observers:
    """Two owned threads; a slow Ping cannot delay the committed-event probe."""

    def __init__(self, state, output, pid, interval):
        if not math.isfinite(interval) or not 0.05 <= interval <= 10:
            raise ValueError("Observer interval must be between 0.05 and 10 seconds")
        self.state, self.output = Path(state), Path(output)
        self.pid, self.interval = pid, interval
        self.stop_event = threading.Event()
        self.threads = []
        self.reports = {}
        self.errors = []
        self.failures = []
        self.stopped = False

    @property
    def failure(self):
        return "; ".join(self.errors) or None

    def start(self):
        if self.threads:
            raise RuntimeError("Observers already started")
        for kind, target in (("control", self.control), ("progress", self.progress)):
            thread = Worker(self.observe, args=(kind, target), stop=self.stop_event)
            self.threads.append(thread)
            try:
                thread.start()
            except BaseException:
                self.threads.pop()
                with finalizer(self.stop):
                    raise

    def observe(self, kind, target):
        before, cpu = time.monotonic_ns(), time.thread_time_ns()
        self.reports[kind] = dict(samples=0)
        try:
            count = target()
            self.reports[kind] = dict(samples=count)
        except Exception as error:
            self.reports[kind]["error"] = str(error)
            self.stop_event.set()
            raise
        finally:
            self.reports[kind].update(
                start_monotonic_ns=before,
                end_monotonic_ns=time.monotonic_ns(),
                cpu_ns=time.thread_time_ns() - cpu,
            )

    def control(self):
        count = 0
        with AppendStream.create_new(self.output / "fast-control.jsonl", durable=False) as stream:
            while not self.stop_event.is_set():
                status, row = ping(self.state / "runtime/control.sock", self.pid)
                row.update(
                    probe_index=count,
                    status=status,
                    monotonic_implementation=time.get_clock_info("monotonic").implementation,
                )
                stream.append_jsonl(row)
                if "error" in row:
                    raise RuntimeError(row["error"])
                count += 1
                self.reports["control"]["samples"] = count
                if self.stop_event.wait(self.interval):
                    break
        return count

    def progress(self):
        count = 0
        path = self.state / "runtime/runtime.sqlite"
        with closing(sqlite3.connect(f"file:{path}?mode=ro", uri=True, timeout=0.05)) as db:
            db.execute("PRAGMA query_only=ON")
            db.execute("PRAGMA cache_size=-1024")
            with AppendStream.create_new(
                self.output / "fast-progress.jsonl", durable=False
            ) as stream:
                while not self.stop_event.is_set():
                    before = time.monotonic_ns()
                    # One statement snapshot, one row, no transaction held across wait.
                    with closing(
                        db.execute(
                            "SELECT (SELECT coalesce(max(seq),0) FROM events),active FROM runtime_counts WHERE id=1"
                        )
                    ) as cursor:
                        seq, active = cursor.fetchone()
                    after = time.monotonic_ns()
                    storage_before = time.monotonic_ns()
                    database_bytes = path.stat().st_size
                    try:
                        wal_bytes = path.with_name(path.name + "-wal").stat().st_size
                    except FileNotFoundError:
                        wal_bytes = None
                    storage_after = time.monotonic_ns()
                    stream.append_jsonl(
                        dict(
                            probe_index=count,
                            before_monotonic_ns=before,
                            after_monotonic_ns=after,
                            event_seq=seq,
                            active=active,
                            storage_before_monotonic_ns=storage_before,
                            storage_after_monotonic_ns=storage_after,
                            database_bytes=database_bytes,
                            wal_bytes=wal_bytes,
                        )
                    )
                    count += 1
                    self.reports["progress"]["samples"] = count
                    if self.stop_event.wait(self.interval):
                        break
        return count

    def check(self):
        if self.failures:
            raise self.failures[0]
        for thread in self.threads:
            thread.check()

    def join(self, thread):
        try:
            thread.join(timeout=15)
        except Exception as error:
            if getattr(error, "owned_cleanup_uncertain", False):
                raise
            # Optional observation failure is reported after the independent
            # workload audit; preserve its native cause until that decision.
            self.failures.append(error)
            self.errors.append(f"{type(error).__name__}: {error}")

    def stop(self):
        if self.stopped:
            return
        self.stop_event.set()
        from release.progress import cleanup_all

        cleanup_all(*(lambda thread=thread: self.join(thread) for thread in self.threads))
        report = dict(
            interval_seconds=self.interval,
            threads=self.reports,
            errors=self.errors,
            cadence="Wait after each sample; no catch-up bursts. Threads are independent.",
        )
        self.stopped = True
        try:
            atomic_write_text(
                self.output / "observer-accounting.json", json.dumps(report, indent=2) + "\n"
            )
        except OSError as error:
            event("observation_failed", error=error, phase="shutdown", partial=True)
            self.errors.append(f"accounting: {type(error).__name__}: {error}")
            self.failures.append(error)
        return report

    def summarize(self):
        self.check()
        if self.failure:
            raise RuntimeError(self.failure)
        result = summarize(self.output / "fast-control.jsonl", self.pid)
        if result["socket"]["seconds"]["count"] != self.reports["control"]["samples"]:
            raise RuntimeError("Control observer sample count disagrees")
        atomic_write_text(
            self.output / "fast-control-summary.json", json.dumps(result, indent=2) + "\n"
        )
        return result
