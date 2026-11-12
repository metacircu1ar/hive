"""Typed evidence rows at the harness's JSON and database boundaries."""

import json
from collections import defaultdict
from typing import Any, TypedDict


class EventIdentity(TypedDict):
    seq: int
    execution: str | None
    data: dict[str, Any]


class EventRow(EventIdentity, total=False):
    at: int


class TraceIdentity(TypedDict):
    kind: str
    process: str
    pid: int
    ns: int


class TraceRow(TraceIdentity, total=False):
    compression: str
    runtime: bool
    thread: str
    agent: str
    turn: str
    message: str
    status: str
    overlapped: bool
    threads: list[str]


def index_events(rows, *, by_agent=False, include_at=False):
    """Group ordered SQLite event rows without changing their identity or order."""
    grouped = defaultdict(lambda: defaultdict(list)) if by_agent else defaultdict(list)
    for row in rows:
        event: EventRow = {
            "seq": row["seq"],
            "execution": row["execution"],
            "data": json.loads(row["data"]),
        }
        if include_at:
            event["at"] = row["at"]
        target = grouped[row["agent"]] if by_agent else grouped
        target[row["kind"]].append(event)
    return grouped
