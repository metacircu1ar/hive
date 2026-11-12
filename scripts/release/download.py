"""Download through an owned worker, then verify immutable bytes before use."""

from pathlib import Path
import sys
from .archive import json_bytes
from .build_record import bytes_record, metadata, read
from .process import json_reply, run
from .temporary import directory


def fetch(url, expected, *, limit=256 * 1024 * 1024, seconds=60, local=False):
    metadata({"sha256": expected, "bytes": 0})
    with directory(prefix="hive-fetch-") as work:
        destination = work / "response"
        request = dict(url=url, limit=limit, seconds=seconds, local=local)
        receipt = json_reply(
            run(
                [
                    sys.executable,
                    str(Path(__file__).with_name("download_worker.py")),
                    str(destination),
                ],
                input=json_bytes(request),
                timeout=seconds + 5,
                limit=65536,
            )
        )
        metadata(receipt)
        data = read(destination, limit=limit)
        if bytes_record(data) != receipt or receipt["sha256"] != expected:
            raise ValueError("Downloaded bytes differ from their declared SHA-256 pin")
    return data
