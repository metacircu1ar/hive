"""Nested scenario owners need time to retire their separately grouped children."""

from release.process import run as _run

# Two15s observer joins, a15s sampler join,60s stop request,60s daemon wait,
# ownership retirement and10s provider observation fit within this outer budget.
SCENARIO_STOP_GRACE = 240


def run(command, **options):
    return _run(command, stop_grace=SCENARIO_STOP_GRACE, **options)
