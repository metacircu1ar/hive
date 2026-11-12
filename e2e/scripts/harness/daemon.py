"""Bounded JSON control requests with one monotonic deadline."""

import json
import socket
import time
from pathlib import Path
from typing import Any
from release.cancellation import check

MAX_RESPONSE_BYTES = 16 * 1024 * 1024


def remaining_seconds(deadline: float) -> float:
    check()
    left = deadline - time.monotonic()
    if left <= 0:
        raise TimeoutError("Control probe deadline exceeded")
    return left


def control_request(
    path: Path,
    request: dict[str, Any],
    *,
    timeout: float = 45,
    deadline: float | None = None,
    observation: dict[str, Any] | None = None,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout if deadline is None else deadline
    observation = {} if observation is None else observation
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(min(0.1, remaining_seconds(deadline)))
        client.connect(str(path))
        observation["connected_monotonic_ns"] = time.monotonic_ns()
        payload = memoryview((json.dumps(request) + "\n").encode())
        while payload:
            client.settimeout(min(0.1, remaining_seconds(deadline)))
            try:
                sent = client.send(payload)
            except TimeoutError:
                continue
            if not sent:
                raise RuntimeError("Control connection ended during submission")
            payload = payload[sent:]
        observation["sent_monotonic_ns"] = time.monotonic_ns()
        data = bytearray()
        while True:
            client.settimeout(min(0.1, remaining_seconds(deadline)))
            try:
                chunk = client.recv(min(65536, MAX_RESPONSE_BYTES - len(data)))
            except TimeoutError:
                continue
            if not chunk:
                raise RuntimeError("Control response ended without a newline")
            observation.setdefault("first_byte_monotonic_ns", time.monotonic_ns())
            data.extend(chunk)
            if len(data) >= MAX_RESPONSE_BYTES:
                raise RuntimeError("Control response exceeds byte limit")
            if b"\n" in chunk:
                break
        observation["received_monotonic_ns"] = time.monotonic_ns()
        if data.count(b"\n") != 1 or not data.endswith(b"\n"):
            raise RuntimeError("Unexpected trailing control response data")
        return json.loads(data)
