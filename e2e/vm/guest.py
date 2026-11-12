"""Bounded guest commands; the local owner also watches host contention."""

import json
import subprocess
import time

from e2e.vm import client
from release.cancellation import bounded, pause
from release.files import atomic_write_bytes, create_directory
from release.progress import finalizer

WATCH_SECONDS = 15
OUTPUT_BYTES = 31 * 1024**2


class Guest:
    def __init__(self, owner, output, guard):
        self.owner, self.logs, self.guard = owner, output / "logs", guard
        self.instance, self.count = owner.name, 0
        create_directory(self.logs, exist_ok=True)

    def run(
        self, name, script, *, root=False, args=(), timeout=120, require=True, keep_output=True
    ):
        self.guard()
        row = self.owner.inspect()
        if row is None or row["status"] != "Running":
            raise RuntimeError("A guest command requires the exact owned running VM")
        self.count += 1
        command = ["sudo", "--non-interactive"] if root else []
        command += [
            "timeout",
            "--signal=TERM",
            "--kill-after=10s",
            str(timeout),
            "bash",
            "-s",
            "--",
            *args,
        ]
        argv = ["limactl", "shell", "--workdir", "/", self.instance, *command]
        session = (
            ""
            if root
            else (
                'export HIVE_VM_SESSION_XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR-<unset>}"\n'
                'export XDG_RUNTIME_DIR="/run/user/$(id -u)"\n'
            )
        )
        body = f'{session}hive_vm_step() {{\n{script}\n}}\nhive_vm_step "$@" </dev/null\n'
        if len(body.encode()) > client.MAX_INPUT:
            raise ValueError("Guest command script exceeds its input bound")
        log = self.logs / f"{self.count:03d}-{name}.log"
        print(json.dumps(dict(step=name, status="started")), flush=True)
        started = time.monotonic()
        until = started + bounded(timeout + 15)
        next_watch = started + WATCH_SECONDS
        process, done = None, None

        def retain():
            stdout = b"" if process is None else bytes(process.output)
            stderr = b"" if process is None else bytes(process.errors)
            status = None if done is None else done.returncode
            header = f"# {name} root={root}\n{body}\n# exit {status}\n## stdout\n".encode()
            atomic_write_bytes(log, header + stdout + b"\n## stderr\n" + stderr)

        self.owner.admission.advance("executing")
        with finalizer(retain):
            with client.spawn(
                argv,
                lock_fd=self.owner.admission.lock_fd,
                evidence=self.owner.request(),
                timeout=timeout,
                limit=OUTPUT_BYTES,
                env=self.owner.env,
                stdin=subprocess.PIPE,
            ) as (process, completion):
                process.send(body.encode(), close=True)
                while not process.exited():
                    now = time.monotonic()
                    if now >= until:
                        raise subprocess.TimeoutExpired(argv, timeout)
                    if now >= next_watch:
                        self.guard()
                        next_watch = now + WATCH_SECONDS
                    pause(0.02)
                status = process.wait(timeout=2)
                done = subprocess.CompletedProcess(
                    argv,
                    status,
                    bytes(process.output).decode(errors="replace"),
                    bytes(process.errors).decode(errors="replace"),
                )
                completion.result(done, timeout)
                status = done.returncode
                if status in (124, 137):
                    raise RuntimeError("Guest command hit its timeout or was killed")
                if require:
                    from release.process import checked

                    checked(done)
        self.owner.check_home()
        self.guard()
        seconds = round(time.monotonic() - started, 2)
        print(json.dumps(dict(step=name, returncode=status, seconds=seconds)), flush=True)
        record = dict(returncode=status, seconds=seconds, log=log.name)
        if keep_output:
            record.update(stdout=done.stdout, stderr=done.stderr)
        return record, done

    def value(self, name, script, **options):
        return self.run(name, script, **options)[1].stdout
