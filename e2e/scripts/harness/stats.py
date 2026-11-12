"""Exact sample and histogram summaries used by evidence reports."""

import math
from collections.abc import Iterable, Mapping


def stats(values: Iterable[float]) -> dict[str, int | float]:
    values = sorted(values)
    if not values:
        return dict(count=0)
    return dict(
        count=len(values),
        min=values[0],
        max=values[-1],
        mean=sum(values) / len(values),
        **{f"p{p}": values[math.ceil(len(values) * p / 100) - 1] for p in (50, 95, 99)},
    )


def distribution(histogram: Mapping[int, int]) -> dict[str, int | float]:
    count = sum(histogram.values())
    if not count:
        return dict(count=0)
    result = dict(
        count=count, mean=sum(v * n for v, n in histogram.items()) / count, max=max(histogram)
    )
    result.update(histogram_values(histogram, {"p50": count * 0.5, "p95": count * 0.95}))
    return result


def histogram_summary(histogram: Mapping[int, int]) -> dict[str, int | float]:
    count = sum(histogram.values())
    result = {"count": count, "mean": sum(k * v for k, v in histogram.items()) / count}
    thresholds = {
        name: max(1, math.ceil(count * fraction))
        for name, fraction in (("p50", 0.5), ("p95", 0.95), ("p99", 0.99), ("max", 1))
    }
    result.update(histogram_values(histogram, thresholds))
    return result


def histogram_values(
    histogram: Mapping[int, int], thresholds: Mapping[str, float]
) -> dict[str, int]:
    ordered = sorted(histogram.items())
    result = {}
    for name, threshold in thresholds.items():
        total = 0
        for value, occurrences in ordered:
            total += occurrences
            if total >= threshold:
                result[name] = value
                break
    return result
