"""One admitted guest context, clean source inputs and supplemental host guard."""

from contextlib import contextmanager
from contextvars import ContextVar
from functools import partial
import json
from pathlib import Path
import pwd
import os
import shutil

from e2e.ci.container import source_file
from e2e.ci.steps import ci_env
from e2e.docker import Docker
from e2e.paths import product_root
from e2e.vm.images import MIN_FREE
from release.files import atomic_write_text
from release.process import run

_active = ContextVar("vm_owner", default=None)


def current():
    owner = _active.get()
    if owner is None:
        raise RuntimeError("Guest validation requires an admitted owner")
    return owner


@contextmanager
def bind(owner):
    token = _active.set(owner)
    try:
        yield
    finally:
        _active.reset(token)


def emit(**fields):
    print(json.dumps(fields), flush=True)


def fail(message, **fields):
    emit(error=message, **fields)
    raise RuntimeError(message)


def host_run(argv, *, timeout=60, log=None):
    owner = current()
    if argv[0] == "limactl":
        done = owner.call(argv[1:], timeout=timeout, limit=64 * 1024**2)
    else:
        done = run(
            argv,
            timeout=timeout,
            check=True,
            text=True,
            env=ci_env(),
            pass_fds=(owner.admission.lock_fd,),
            limit=64 * 1024**2,
        )
    if log is not None:
        atomic_write_text(
            log, f"# exit {done.returncode}\n## stdout\n{done.stdout}\n## stderr\n{done.stderr}"
        )
    return done.stdout


def guest_script(name):
    owner = current()
    if name not in ("provision.sh", "host-policy.sh"):
        raise ValueError("Unknown guest fixture script")
    return source_file(
        product_root(),
        owner.state["source_commit"],
        "e2e/vm/" + name,
        native=partial(run, pass_fds=(owner.admission.lock_fd,)),
    ).decode()


def free_bytes():
    return shutil.disk_usage(Path(pwd.getpwuid(os.getuid()).pw_dir)).free


def activity(native=run):
    done = native(
        ["ps", "-axo", "pid=,command="],
        text=True,
        check=True,
        timeout=30,
        env=ci_env(),
        limit=4 * 1024**2,
    )
    found = []
    for line in done.stdout.splitlines():
        pid, _, command = line.strip().partition(" ")
        words = command.split()
        if words and Path(words[0]).name == "cargo" and int(pid) != os.getpid():
            found.append(dict(kind="cargo", pid=int(pid)))
    if shutil.which("docker") is not None:
        docker = Docker.resolve(native=native)
        rows = docker.call(
            ["container", "ls", "--format", "{{.Names}}"], timeout=30
        ).stdout.splitlines()
        found.extend(dict(kind="container") for name in rows if name.startswith("hive-"))
    return found


def guard():
    owner = current()
    owner.check_home()
    observed = activity(native=partial(run, pass_fds=(owner.admission.lock_fd,)))
    if observed:
        fail("Another host build or container is active", activity=observed)
    available = free_bytes()
    if available < MIN_FREE:
        fail("The VM host needs at least 10 GiB free", free_bytes=available)


def lima_facts(status):
    owner = current()
    config = status["config"]
    return dict(
        version=host_run(["limactl", "--version"]).strip(),
        image=owner.state["config"]["images"],
        requested=dict(memory_gib=owner.state["memory_gib"], cpus=4, disk_gib=40),
        plain=config.get("plain"),
        mounts=config.get("mounts"),
        port_forwards=config.get("portForwards"),
        containerd={key: config.get("containerd", {}).get(key) for key in ("system", "user")},
        vm_type=status.get("vmType"),
        cpus=status.get("cpus"),
        memory_bytes=status.get("memory"),
        disk_bytes=status.get("disk"),
        user=config.get("user"),
    )
