"""JSON CLI calls and cursor pagination shared by scenario drivers."""

from release.process import run as run_process

from contextlib import contextmanager
from contextvars import ContextVar
import time
from collections.abc import Callable
from typing import Any

from .checks import require
from release.process import json_reply

_deadline = ContextVar("scenario_cli_deadline", default=None)


@contextmanager
def call_deadline(until):
    previous = _deadline.get()
    token = _deadline.set(until if previous is None else min(until, previous))
    try:
        yield
    finally:
        _deadline.reset(token)


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

    def __call__(self, *arguments: str, allow_list: bool = False) -> dict[str, Any] | list[Any]:
        timeout = self.timeout
        until = _deadline.get()
        if until is not None:
            remaining = until - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("Scenario CLI cleanup budget expired")
            timeout = min(timeout, remaining)
        result = run_process(
            self.argv(*arguments),
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
            stop_grace=12 if arguments[0] in ("auth", "models") else 0.5,
        )
        return json_reply(result, allow_list=allow_list)


def event_pages(call: Callable, agent: str, *, limit: int = 200) -> list[dict[str, Any]]:
    """Read durable events in increasing sequence order, rejecting lost history."""
    return collect_events(
        lambda after: call("read", agent, "--after", str(after), "--limit", str(limit))
    )


def collect_events(fetch: Callable, *, max_pages: int | None = None) -> list[dict[str, Any]]:
    events, after, pages = [], 0, 0
    while max_pages is None or pages < max_pages:
        page = fetch(after)
        require(not page.get("history_expired"), "Scenario history expired")
        events.extend(page["events"])
        if not page["events"]:
            return events
        cursor = page["cursor"]
        require(cursor > after, "Event cursor did not advance")
        after = cursor
        pages += 1
    raise RuntimeError("Unexpected scenario event count")


def read_agents(call):
    """Follow insertion cursors; historical agents must not hide this run."""
    rows, after = [], 0
    while True:
        page = call("agents", "--after", str(after), "--limit", "200")
        rows.extend(page["agents"])
        cursor = page.get("next_after")
        if cursor is None:
            return rows
        if cursor <= after:
            raise RuntimeError("Agent pagination cursor did not advance")
        after = cursor
