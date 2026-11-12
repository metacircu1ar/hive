"""Small acceptance helpers for installed release archives and deterministic providers."""

import json
import signal
from contextlib import contextmanager
import time
from collections.abc import Callable
from typing import Any
from .process import ProcessFailure, json_reply, run
from .owned_process import spawn
from .cancellation import bounded, check, deadline as within, pause
from .files import raw_log
from .terminal import terminal


def require(condition, detail):
    if not condition:
        raise RuntimeError(detail)


class Cli:
    def __init__(
        self, command: list[str], *, timeout: int = 45, prefixes: dict[str, list[str]] | None = None
    ):
        self.command = command
        self.prefixes = prefixes or {}
        self.timeout = timeout

    def argv(self, *arguments: str) -> list[str]:
        """Choose an explicitly configured command-family prefix."""
        return self.prefixes.get(arguments[0], self.command) + list(arguments)

    def __call__(self, *arguments: str) -> dict[str, Any]:
        result = run(
            self.argv(*arguments),
            capture_output=True,
            text=True,
            timeout=self.timeout,
            check=False,
            stop_grace=12,
        )
        return json_reply(result)


def event_pages(call: Callable, agent: str, *, limit: int = 200) -> list[dict[str, Any]]:
    """Read complete durable history and reject expired or nonadvancing pages."""
    events, after = [], 0
    while True:
        page = call("read", agent, "--after", str(after), "--limit", str(limit))
        require(not page.get("history_expired"), "Installed agent history expired")
        events.extend(page["events"])
        if not page["events"]:
            return events
        require(page["cursor"] > after, "Event cursor did not advance")
        after = page["cursor"]


def terminal_json(output):
    """A PTY merges human diagnostics with the response, which starts a line."""
    lines = output.splitlines(keepends=True)
    for index, line in enumerate(lines):
        if line.startswith(b"{"):
            return json.loads(b"".join(lines[index:]))
    raise ValueError("Terminal output has no JSON response")


def event_data(call, data, *, limit=1024 * 1024):
    """Hydrate bounded artifact-backed event JSON, checking byte progress and EOF."""
    if "artifact" not in data:
        return data
    artifact = data["artifact"]
    size = artifact["bytes"]
    require(type(size) is int and 0 < size <= limit, "Event artifact exceeds its bound")
    chunks, offset = [], 0
    while offset < size:
        page = call("output", artifact["id"], "--offset", str(offset), "--limit", "65536")
        text, next_offset = page["text"], page["next_offset"]
        require(
            isinstance(text, str)
            and type(next_offset) is int
            and offset < next_offset <= size
            and len(text.encode()) == next_offset - offset,
            "Artifact byte cursor did not advance correctly",
        )
        require(
            type(page["eof"]) is bool and page["eof"] == (next_offset == size),
            "Artifact EOF disagrees with its size",
        )
        chunks.append(text)
        offset = next_offset
    result = json.loads("".join(chunks))
    require(isinstance(result, dict), "Event artifact must contain an object")
    return result


def mock_login(command, *, env=None):
    """Use the real registration path with an explicitly selected fake provider."""
    check()
    with terminal() as (master, slave, retire_capture):
        with spawn(
            command + ["auth", "login", "mock"],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=env,
            stop_grace=12,
            capture_fd=master,
            capture_close=retire_capture,
            limit=1024 * 1024,
        ) as process:
            until = time.monotonic() + bounded(30)
            while time.monotonic() < until:
                if process.exited():
                    break
                pause(0.02)
            require(process.wait(timeout=5) == 0, "Mock registration failed")
            require(
                terminal_json(process.output)["state"] == "authenticated",
                "Mock registration failed",
            )


def wait_for(probe, label, process=None, seconds=45):
    with within(seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            check()
            if process is not None:
                require(not process.exited(), "Owned process exited before readiness")
            result = probe()
            if result:
                return result
            pause(0.05)
    raise RuntimeError("Readiness did not complete before its deadline")


def ready(call, *args, **kwargs):
    """Only explicit connection-unavailable responses are transient startup state."""
    try:
        return call(*args, **kwargs)
    except ProcessFailure as failure:
        try:
            value = json.loads(failure.result.stdout)
        except (ValueError, TypeError):
            raise failure from None
        code = value.get("error", {}).get("code") if isinstance(value, dict) else None
        if code not in {"daemon_unavailable", "comb_unavailable"}:
            raise
        return None


@contextmanager
def running(command, log_path, env):
    with raw_log(log_path) as log:
        with spawn(
            command,
            stdout=log,
            stderr=log,
            env=env,
            stop_signal=signal.SIGINT,
            stop_grace=20,
            limit=64 * 1024 * 1024,
        ) as process:
            yield process
        require(process.returncode == 0, "Owned service did not stop successfully")


def json_command(command, *, env, body=None, allow_list=False):
    result = run(
        command,
        input=body,
        capture_output=True,
        text=True,
        env=env,
        timeout=30,
        check=False,
        stop_grace=12,
    )
    return json_reply(result, allow_list=allow_list)
