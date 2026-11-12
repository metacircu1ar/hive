"""Available Linux memory, bounded by mounts in the process's own namespace."""

from pathlib import Path


def _path(text):
    value = text.replace("\\040", " ").replace("\\011", "\t")
    value = value.replace("\\012", "\n").replace("\\134", "\\")
    path = Path(value)
    if not path.is_absolute() or ".." in path.parts:
        raise ValueError("Cannot resolve the current cgroup memory limits")
    return path


def _mount(line):
    before, after = line.split(" - ", 1)
    fields, tail = before.split(), after.split()
    if len(fields) < 6 or len(tail) < 3:
        raise ValueError("Invalid cgroup mount information")
    if tail[0] == "cgroup2":
        kind = "v2"
    elif tail[0] == "cgroup" and "memory" in tail[2].split(","):
        kind = "v1"
    else:
        return None
    return kind, _path(fields[3]), _path(fields[4])


def _number(text):
    value = int(text.strip())
    if not 0 <= value <= (1 << 64) - 1:
        raise ValueError("Invalid cgroup memory quantity")
    return value


def _bound(mount, member, available):
    kind, root, point = mount
    if not member.is_relative_to(root):
        return None
    candidate = point / member.relative_to(root)
    if not candidate.is_dir():
        return None
    maximum, used = (
        ("memory.max", "memory.current") if kind == "v2"
        else ("memory.limit_in_bytes", "memory.usage_in_bytes")
    )
    directory = candidate
    while True:
        try:
            text = (directory / maximum).read_text().strip()
        except FileNotFoundError:
            text = "max"  # An unbounded root has no memory.max.
        if text != "max":
            usage = _number((directory / used).read_text())
            working = max(0, usage - _file_cache(directory, kind))
            available = min(available, max(0, _number(text) - working))
        if directory == point:
            return available
        directory = directory.parent


def _file_cache(directory, kind):
    prefix = "total_" if kind == "v1" else ""
    keys = {prefix + "active_file", prefix + "inactive_file"}
    values = {}
    for line in (directory / "memory.stat").read_text().splitlines():
        fields = line.split()
        if fields and fields[0] in keys:
            if len(fields) != 2 or fields[0] in values:
                raise ValueError("Invalid cgroup file-cache statistics")
            values[fields[0]] = _number(fields[1])
    if values.keys() != keys:
        raise ValueError("Missing cgroup file-cache statistics")
    # File LRU excludes shmem/tmpfs; the broader `file` counter does not.
    return _number(str(sum(values.values())))


def available(proc=Path("/proc")):
    fields = dict(line.split(":", 1) for line in (proc / "meminfo").read_text().splitlines())
    raw = fields.get("MemAvailable", "").split()
    if len(raw) != 2 or raw[1] != "kB":
        raise ValueError("Cannot establish available Linux memory")
    available = _number(raw[0]) * 1024
    mounts = [_mount(line) for line in (proc / "self/mountinfo").read_text().splitlines()]
    mounts = [mount for mount in mounts if mount is not None]
    for line in (proc / "self/cgroup").read_text().splitlines():
        _, controllers, text = line.split(":", 2)
        if not controllers:
            kind = "v2"
        elif "memory" in controllers.split(","):
            kind = "v1"
        else:
            continue
        member, memory, resolved = _path(text), False, False
        for mount in mounts:
            if mount[0] != kind:
                continue
            if kind == "v2":
                names = (mount[2] / "cgroup.controllers").read_text().split()
                if "memory" not in names:
                    continue
            memory = True
            bounded = _bound(mount, member, available)
            if bounded is not None:
                available, resolved = bounded, True
        if (memory and not resolved) or (kind == "v1" and not memory):
            raise ValueError("Cannot resolve the current cgroup memory limits")
    return available
