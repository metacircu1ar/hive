"""Fixed planned arrivals with explicit late attempts and acknowledgement brackets."""

from release.cancellation import pause

import json
import time


def submit(stream, epoch_ns, rate, offset, count, call, check_running, monitor):
    scheduled_ns = epoch_ns + round(offset / rate * 1e9) if rate else None
    if scheduled_ns is not None:
        while (remaining := (scheduled_ns - time.monotonic_ns()) / 1e9) > 0:
            check_running()
            # Full monitoring can overrun, but never deliberately start it at a
            # deadline. Past deadlines catch up sequentially without dropping work.
            if remaining > 2:
                monitor()
            pause(min(max(0, (scheduled_ns - time.monotonic_ns()) / 1e9), 0.5))
    check_running()  # Includes overdue/catch-up and burst arrivals.
    row = dict(
        offset=offset,
        count=count,
        scheduled_monotonic_ns=scheduled_ns,
        started_monotonic_ns=time.monotonic_ns(),
        acknowledged=False,
    )
    try:
        results = call()
        if len(results) != count:
            raise RuntimeError("Incomplete seed batch")
        row["acknowledged"] = True
        return results
    finally:
        row["finished_monotonic_ns"] = time.monotonic_ns()
        stream.append_jsonl(row)
