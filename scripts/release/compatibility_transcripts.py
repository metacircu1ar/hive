"""Replay frozen successful/negative envelopes against their producing binaries."""

import json

from .acceptance import require
from .unix_transport import exchange


def expected(reply, pointers):
    for pointer, value in pointers.items():
        selected = reply
        for part in pointer.lstrip("/").split("/"):
            part = part.replace("~1", "/").replace("~0", "~")
            if isinstance(selected, list):
                require(
                    part.isascii() and part.isdecimal() and str(int(part)) == part,
                    "Invalid transcript array index",
                )
                selected = selected[int(part)]
            else:
                selected = selected[part]
        require(selected == value, "Frozen transcript expectation changed: " + pointer)


def control(address, cases):
    for case in cases:
        expected(exchange(address, case["request"]), case["expect"])


def service(address, connection, cases):
    credential = json.loads(connection.read_bytes())["credential"]
    for case in cases:
        request = {
            "protocol": case["protocol"],
            "credential": credential,
            "claim": None,
            "operation": case["operation"],
        }
        expected(exchange(address, request), case["expect"])
