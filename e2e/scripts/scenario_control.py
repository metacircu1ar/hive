"""Bounded current control probes and independent timing validation."""

import json
import time
import uuid
from e2e.scripts.harness.io import CompleteLines
from e2e.scripts.status_schema import PING, WRITER_COMMITTED, WRITER_CURRENT, validate
from e2e.scripts.harness.daemon import control_request, remaining_seconds
from e2e.scripts.harness.stats import stats


def validate_status(value, expected_pid):
    writer = value.get("writer_observation") if isinstance(value, dict) else None
    source = writer.get("source") if isinstance(writer, dict) else None
    if source not in ("current_transaction", "committed"):
        raise RuntimeError("Invalid control writer observation")
    schema = {
        **PING,
        "writer_observation": WRITER_CURRENT
        if source == "current_transaction"
        else WRITER_COMMITTED,
    }
    if "managed_service" in value:
        schema["managed_service"] = str
    if "comb" not in value.get("compiled_features", ["comb"]):
        schema["comb_backlog"] = {"collectors": "u64", "rows": "u64", "delivery_supported": bool}
    validate(value, schema, "control status")
    if value["compiled_features"] not in (
        [],
        ["comb"],
        ["barrier-delay"],
        ["comb", "barrier-delay"],
    ):
        raise RuntimeError("Invalid compiled features")
    if "comb" not in value["compiled_features"] and value["comb_backlog"]["delivery_supported"]:
        raise RuntimeError("Core cannot deliver Comb backlog")
    if value["pid"] != expected_pid or expected_pid <= 0:
        raise RuntimeError("Invalid control status response")


def ping(path, expected_pid, timeout=40):
    """One fresh connection and one deadline; retain failures as observations."""
    request_id = str(uuid.uuid4())
    observation = dict(kind="socket", request_id=request_id, start_monotonic_ns=time.monotonic_ns())
    deadline = time.monotonic() + timeout
    value = None
    try:
        value = control_request(
            path,
            dict(protocol=1, id=request_id, args=dict(op="ping")),
            deadline=deadline,
            observation=observation,
        )
        validate_status(value, expected_pid)
        remaining_seconds(deadline)
    except (OSError, ValueError, RuntimeError) as error:
        observation["error"] = f"{type(error).__name__}: {error}"
        value = None
    observation["end_monotonic_ns"] = time.monotonic_ns()
    observation["seconds"] = (
        observation["end_monotonic_ns"] - observation["start_monotonic_ns"]
    ) / 1e9
    return value, observation


def summarize(path, expected_pid):
    values = {kind: {} for kind in ("cli", "socket")}
    previous_start = None
    with CompleteLines(path) as stream:
        for index, line in enumerate(stream):
            row = json.loads(line)
            if "error" in row or row["probe_index"] != index:
                raise RuntimeError("Failed or missing control observations")
            validate_status(row["status"], expected_pid)
            names = (
                ["start", "end"]
                if row["kind"] == "cli"
                else ["start", "connected", "sent", "first_byte", "received", "end"]
            )
            stamps = [row[name + "_monotonic_ns"] for name in names]
            if not all(type(stamp) is int and stamp >= 0 for stamp in stamps) or stamps != sorted(
                stamps
            ):
                raise RuntimeError("Invalid client control timestamps")
            sample = dict(seconds=(stamps[-1] - stamps[0]) / 1e9)
            if previous_start is not None:
                if stamps[0] < previous_start:
                    raise RuntimeError("Control request clock regressed")
                sample["start_interval_seconds"] = (stamps[0] - previous_start) / 1e9
            previous_start = stamps[0]
            for name, value in sample.items():
                values[row["kind"]].setdefault(name, []).append(value)
    if previous_start is None:
        raise RuntimeError("Missing control observations")

    return {
        kind: {name: stats(samples) for name, samples in metrics.items()}
        for kind, metrics in values.items()
    }
