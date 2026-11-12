"""Names used to align the daemon and Python monotonic clocks."""

MONOTONIC_IMPLEMENTATIONS = {
    "CLOCK_UPTIME_RAW": "mach_absolute_time()",
    "CLOCK_MONOTONIC": "clock_gettime(CLOCK_MONOTONIC)",
}
