"""Collect current bounded tracing logs; never claim exhaustive profiling coverage."""

import hashlib
import json
import resource
import time
from pathlib import Path

from release.files import AppendStream, atomic_write_json
from e2e.scripts.harness.io import CompleteLines

EVENTS = {
    "writer.batch",
    "runtime.timing",
    "state.sync",
    "provider.capacity_drain",
    "state.barrier_delay",
}
# Records, not intervals: no duration_ns to validate.
RECORDS = {"provider.capacity_drain", "state.barrier_delay"}


def cpu_snapshot():
    value = resource.getrusage(resource.RUSAGE_SELF)
    return dict(
        monotonic_ns=time.monotonic_ns(), user_seconds=value.ru_utime, system_seconds=value.ru_stime
    )


def _segments(directory):
    current = directory / "hive-daemon.jsonl"
    paths = []
    for path in directory.glob(".hive-daemon.jsonl.*.log"):
        number = path.name.removeprefix(".hive-daemon.jsonl.").removesuffix(".log")
        if number.isdigit():
            paths.append((int(number), path))
    return [path for _, path in sorted(paths)] + [current]


def collect(state, output, pid):
    """Stream retained segments after daemon shutdown. Raw segments stay in state."""
    files, counts, loss = [], {}, []
    starts, stops, rotations = [], [], 0
    with AppendStream.create_new(output / "debug-timings.jsonl", durable=False) as stream:
        for path in _segments(Path(state) / "runtime/logs"):
            digest = hashlib.sha256()
            with CompleteLines(path) as lines:
                for line in lines:
                    digest.update(line if isinstance(line, bytes) else line.encode())
                    row = json.loads(line)
                    event = row.get("event")
                    if event == "daemon.starting":
                        starts.append(row.get("pid"))
                    if event == "daemon.stopped":
                        stops.append(row.get("success"))
                    if event in {"log.rotated", "log.recovered"}:
                        rotations += 1
                    if event == "log.dropped" or event in {"log.write_failed", "log.finish_failed"}:
                        loss.append(row)
                    if event not in EVENTS:
                        continue
                    if event not in RECORDS:
                        duration = row.get("duration_ns")
                        if type(duration) is not int or duration < 0:
                            raise ValueError("Invalid debug timing duration")
                        start, end = row.get("start_ns"), row.get("end_ns")
                        if (
                            start is not None
                            and end is not None
                            and not (type(start) is int and type(end) is int and 0 <= start <= end)
                        ):
                            raise ValueError("Invalid debug monotonic interval")
                    counts[event] = counts.get(event, 0) + 1
                    stream.append_jsonl(row)
            files.append(
                dict(
                    path=path.name,
                    bytes=path.stat().st_size,
                    complete_lines_sha256=digest.hexdigest(),
                )
            )
    if starts and starts != [pid]:
        raise ValueError("Debug log daemon identity mismatch")
    result = dict(
        events=counts,
        files=files,
        loss_records=loss,
        rotation_records=rotations,
        observed_start=starts == [pid],
        observed_clean_stop=stops == [True],
        exhaustive=False,
        coverage_note="Bounded best-effort logging: absence of a loss record cannot certify complete delivery. Retention may omit older segments; timings describe retained events only.",
    )
    atomic_write_json(output / "debug-timings-summary.json", result)
    return result
