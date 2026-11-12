"""Streaming evidence readers without whole-file buffering."""

import json
from release.files import CompleteLines as _CompleteLines
from release.cancellation import check
from collections.abc import Iterator
from pathlib import Path
from typing import Any


class CompleteLines(_CompleteLines):
    """The shared complete-record reader with cooperative audit cancellation."""

    def __next__(self):
        check()
        return super().__next__()


def json_lines(path: str | Path) -> Iterator[Any]:
    with CompleteLines(path) as stream:
        for line in stream:
            yield json.loads(line)
