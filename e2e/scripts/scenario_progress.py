"""Bracketed committed-progress screen, independent of control-response latency."""

import json
from e2e.scripts.harness.io import CompleteLines
from statistics import median

from e2e.scripts.harness.checks import require
from e2e.scripts.harness.stats import stats


def summarize(progress_path, control_path):
    rows = read_progress(progress_path)
    positive = [row for row in rows if row["active"] > 0]
    require(len(positive) >= 2, "Missing active progress window")
    begin, end = positive[0]["after_monotonic_ns"], positive[-1]["before_monotonic_ns"]
    require(end > begin, "Invalid active progress window")
    intervals = [
        (b["before_monotonic_ns"] - a["before_monotonic_ns"]) / 1e9 for a, b in zip(rows, rows[1:])
    ]
    windows, median_windows = progress_windows(rows, begin, end)
    typical = median(w["midpoint_rate"] for w in median_windows) if median_windows else None
    flags = [w for w in windows if w["upper_rate"] < typical * 0.3]
    episodes = flagged_episodes(flags)
    gaps = [
        dict(
            before_monotonic_ns=a["after_monotonic_ns"], after_monotonic_ns=b["before_monotonic_ns"]
        )
        for a, b in zip(rows, rows[1:])
        if b["before_monotonic_ns"] - a["after_monotonic_ns"] > 500_000_000
    ]
    wide = [r for r in rows if r["after_monotonic_ns"] - r["before_monotonic_ns"] > 50_000_000]
    zero = zero_progress(rows)
    controls, overlaps, all_count = read_controls(control_path, begin, end)
    return dict(
        progress_samples=len(rows),
        control_samples=all_count,
        active_window=dict(
            begin_monotonic_ns=begin,
            end_monotonic_ns=end,
            definition="Observed interior: first to last positive active-counter sample. Select by request starts, retain full latency. Whole-observer maxima include boundary-uncertain requests outside this interval.",
        ),
        storage_observations=storage_observations(rows),
        progress_interval_seconds=stats(intervals),
        progress_query_seconds=stats(
            [(r["after_monotonic_ns"] - r["before_monotonic_ns"]) / 1e9 for r in rows]
        ),
        eligible_windows=len(windows),
        median_windows=len(median_windows),
        median_window_events_per_second=typical,
        flagged_windows=flags,
        flagged_episodes=episodes,
        observation_gaps=gaps,
        wide_query_brackets=wide,
        zero_progress_intervals=zero,
        active_span_event_rate=active_span_event_rate(positive),
        active_control_seconds=stats(controls),
        active_control_occupied_seconds=sum(controls),
        active_control_start_interval_seconds=stats(
            [
                (b["start_monotonic_ns"] - a["start_monotonic_ns"]) / 1e9
                for a, b in zip(overlaps, overlaps[1:])
            ]
        ),
        rule="Sliding detection windows: every eligible sample, earliest next sample with >=1s guaranteed elapsed, active endpoints, exclude first2s/last5s. Flag rate upper bound <30% non-overlapping-window median midpoint rate; merge overlapping flags. Absolute zero-progress >=1s also requires investigation. Query brackets>50ms or observation gaps>500ms are uncertain coverage and require investigation, not a pass or causal attribution.",
    )


def read_progress(progress_path):
    rows = []
    with CompleteLines(progress_path) as stream:
        for line in stream:
            row = json.loads(line)
            fields = [
                row.get(k)
                for k in (
                    "probe_index",
                    "before_monotonic_ns",
                    "after_monotonic_ns",
                    "event_seq",
                    "active",
                )
            ]
            require(all(type(v) is int and v >= 0 for v in fields), "Invalid progress observation")
            require(row["probe_index"] == len(rows), "Missing progress observation")
            require(
                row["before_monotonic_ns"] <= row["after_monotonic_ns"], "Reversed progress bracket"
            )
            require(
                all(
                    type(row.get(k)) is int
                    for k in (
                        "storage_before_monotonic_ns",
                        "storage_after_monotonic_ns",
                        "database_bytes",
                    )
                )
                and row["database_bytes"] > 0
                and "wal_bytes" in row
                and (
                    row["wal_bytes"] is None
                    or type(row["wal_bytes"]) is int
                    and row["wal_bytes"] >= 0
                ),
                "Invalid storage observation",
            )
            require(
                row["after_monotonic_ns"]
                <= row["storage_before_monotonic_ns"]
                <= row["storage_after_monotonic_ns"],
                "Reversed storage bracket",
            )
            if rows:
                require(
                    rows[-1]["storage_after_monotonic_ns"] <= row["before_monotonic_ns"],
                    "Overlapping storage observations",
                )
                require(
                    rows[-1]["after_monotonic_ns"] <= row["before_monotonic_ns"],
                    "Overlapping progress brackets",
                )
                require(
                    rows[-1]["event_seq"] <= row["event_seq"], "Committed event counter regressed"
                )
            rows.append(row)
    return rows


def progress_windows(rows, begin, end):
    windows, median_windows, right, next_begin = [], [], 0, begin + 2_000_000_000
    for left, first in enumerate(rows):
        if first["before_monotonic_ns"] < begin + 2_000_000_000 or first["active"] == 0:
            continue
        right = max(right, left + 1)
        while (
            right < len(rows)
            and rows[right]["before_monotonic_ns"] - first["after_monotonic_ns"] < 1_000_000_000
        ):
            right += 1
        if right == len(rows):
            break
        last = rows[right]
        if last["after_monotonic_ns"] > end - 5_000_000_000 or last["active"] == 0:
            continue
        delta = last["event_seq"] - first["event_seq"]
        maximum = (last["after_monotonic_ns"] - first["before_monotonic_ns"]) / 1e9
        minimum = (last["before_monotonic_ns"] - first["after_monotonic_ns"]) / 1e9
        windows.append(
            dict(
                first=left,
                last=right,
                before_monotonic_ns=first["before_monotonic_ns"],
                after_monotonic_ns=last["after_monotonic_ns"],
                delta=delta,
                lower_rate=delta / maximum,
                upper_rate=delta / minimum,
                midpoint_rate=2 * delta / (minimum + maximum),
            )
        )
        if first["before_monotonic_ns"] >= next_begin:
            median_windows.append(windows[-1])
            next_begin = last["after_monotonic_ns"]
    return windows, median_windows


def flagged_episodes(flags):
    episodes = []
    for window in flags:
        if episodes and window["before_monotonic_ns"] <= episodes[-1]["after_monotonic_ns"]:
            episodes[-1]["after_monotonic_ns"] = max(
                episodes[-1]["after_monotonic_ns"], window["after_monotonic_ns"]
            )
            episodes[-1]["windows"] += 1
        else:
            episodes.append(
                dict(
                    before_monotonic_ns=window["before_monotonic_ns"],
                    after_monotonic_ns=window["after_monotonic_ns"],
                    windows=1,
                )
            )
    return episodes


def zero_progress(rows):
    zero, anchor = [], None
    previous = rows[0]
    for row in rows:
        if row["active"] == 0 or anchor is None or row["event_seq"] != anchor["event_seq"]:
            if (
                anchor is not None
                and previous["before_monotonic_ns"] - anchor["after_monotonic_ns"] >= 1_000_000_000
            ):
                zero.append(
                    dict(
                        before_monotonic_ns=anchor["after_monotonic_ns"],
                        after_monotonic_ns=previous["before_monotonic_ns"],
                        event_seq=anchor["event_seq"],
                    )
                )
            anchor = row if row["active"] > 0 else None
        previous = row
    if (
        anchor is not None
        and previous["before_monotonic_ns"] - anchor["after_monotonic_ns"] >= 1_000_000_000
    ):
        zero.append(
            dict(
                before_monotonic_ns=anchor["after_monotonic_ns"],
                after_monotonic_ns=previous["before_monotonic_ns"],
                event_seq=anchor["event_seq"],
            )
        )
    return zero


def read_controls(control_path, begin, end):
    controls, overlaps, all_count = [], [], 0
    with CompleteLines(control_path) as stream:
        for line in stream:
            row = json.loads(line)
            require("error" not in row, "Failed control observation")
            start, finish = row["start_monotonic_ns"], row["end_monotonic_ns"]
            require(
                type(start) is int and type(finish) is int and start <= finish,
                "Invalid control bracket",
            )
            all_count += 1
            # Select by START, so a slow reply crossing completion is retained.
            if begin <= start <= end:
                controls.append((finish - start) / 1e9)
                overlaps.append(dict(start_monotonic_ns=start, end_monotonic_ns=finish))
    return controls, overlaps, all_count


def storage_observations(rows):
    return dict(
        count=len(rows),
        missing_wal=sum(r["wal_bytes"] is None for r in rows),
        duration_seconds=stats(
            [
                (r["storage_after_monotonic_ns"] - r["storage_before_monotonic_ns"]) / 1e9
                for r in rows
            ]
        ),
        semantics="Sequential DB/WAL stat observations, not an atomic snapshot, bytes written or checkpoint count. Durations include scheduling delay.",
    )


def active_span_event_rate(positive):
    return (positive[-1]["event_seq"] - positive[0]["event_seq"]) / (
        (
            positive[-1]["before_monotonic_ns"]
            + positive[-1]["after_monotonic_ns"]
            - positive[0]["before_monotonic_ns"]
            - positive[0]["after_monotonic_ns"]
        )
        / 2e9
    )
