"""Hold admission around a CLI; its detached daemons inherit no control fds."""

import argparse
from contextlib import contextmanager
import os
from pathlib import Path
import stat
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
import e2e
from e2e.campaign import cleanup_uncertain
from e2e.vm import proof
from release.build_record import decode
from release.cancellation import check, pause
from release.files import _owned_fd
from release.owned_process import spawn as owned_spawn
from release.process import run as process_run
from release.progress import entry, event, finalizer

STOP_GRACE = 240
MAX_INPUT = 1024 * 1024
MAX_RECEIPT = 4096


def command(argv, timeout, limit, lock_fd, status_fd, evidence=None):
    result = [
        sys.executable,
        str(Path(__file__).resolve()),
        "--timeout",
        str(timeout),
        "--limit",
        str(limit),
        "--status-fd",
        str(status_fd),
    ]
    if lock_fd is not None:
        result += ["--admission-fd", str(lock_fd)]
    if evidence is not None:
        result += ["--completion-file", str(evidence)]
    return result + ["--", *argv]


class Channel:
    def __init__(self, reader):
        self.reader, self.value = reader, None

    def observe(self):
        if self.value is not None:
            return self.value
        try:
            data = os.read(self.reader, MAX_RECEIPT + 1)
        except BlockingIOError:
            return None
        if not data:
            return None
        if len(data) > MAX_RECEIPT or data.count(b"\n") != 1 or not data.endswith(b"\n"):
            error = ValueError("Malformed native-client completion frame")
            error.owned_cleanup_uncertain = True
            raise error
        value = decode(data)
        expected = (
            {"kind", "returncode"}
            if isinstance(value, dict) and value.get("kind") == "completed"
            else {"kind"}
        )
        if (
            not isinstance(value, dict)
            or set(value) != expected
            or value.get("kind") not in ("completed", "timeout", "failed", "uncertain")
        ):
            error = ValueError("Unexpected native-client completion shape")
            error.owned_cleanup_uncertain = True
            raise error
        if value["kind"] == "completed" and (
            type(value["returncode"]) is not int or not -128 <= value["returncode"] <= 255
        ):
            error = ValueError("Invalid native-client return code")
            error.owned_cleanup_uncertain = True
            raise error
        self.value = value
        return value

    @contextmanager
    def protect(self):
        try:
            yield
        except BaseException as error:

            def mark(failure=error):
                previous = getattr(failure, "owned_cleanup_uncertain", False)
                failure.owned_cleanup_uncertain = True
                value = self.observe()
                if value is not None and value["kind"] != "uncertain":
                    failure.owned_cleanup_uncertain = previous

            with finalizer(mark):
                raise

    def result(self, done, timeout):
        value = self.observe()
        if value is None or value["kind"] == "uncertain":
            error = RuntimeError("Native-client cleanup is not confirmed")
            error.owned_cleanup_uncertain = True
            error.result = done
            raise error
        if value["kind"] == "timeout":
            raise subprocess.TimeoutExpired(
                done.args, timeout, output=done.stdout, stderr=done.stderr
            )
        if done.returncode != 0 or value["kind"] == "failed":
            error = RuntimeError("Native-client execution failed before completion")
            error.result = done
            raise error
        done.returncode = value["returncode"]
        return done


@contextmanager
def channel():
    reader, writer = os.pipe()
    with _owned_fd(reader), _owned_fd(writer):
        os.set_blocking(reader, False)
        yield Channel(reader), writer


def descriptors(lock_fd, writer):
    return (writer,) if lock_fd is None else (lock_fd, writer)


def run(
    argv,
    *,
    lock_fd=None,
    native=process_run,
    timeout=30,
    limit=4 * 1024**2,
    evidence=None,
    **options,
):
    from release.process import checked

    requested_check = options.pop("check", False)
    with channel() as (completion, writer), completion.protect():
        done = native(
            command(argv, timeout, limit, lock_fd, writer, evidence),
            timeout=timeout + 5,
            limit=limit,
            pass_fds=descriptors(lock_fd, writer),
            stop_grace=STOP_GRACE,
            check=False,
            **options,
        )
        completion.result(done, timeout)
        if requested_check:
            checked(done)
        return done


@contextmanager
def spawn(argv, *, lock_fd=None, timeout, limit, evidence=None, **options):
    with channel() as (completion, writer), completion.protect():
        with owned_spawn(
            command(argv, timeout, limit, lock_fd, writer, evidence),
            limit=limit,
            pass_fds=descriptors(lock_fd, writer),
            stop_grace=STOP_GRACE,
            **options,
        ) as process:
            yield process, completion


def input_body(timeout):
    data = bytearray()
    until = time.monotonic() + min(30, timeout)
    os.set_blocking(sys.stdin.fileno(), False)
    while True:
        check()
        if time.monotonic() >= until:
            raise TimeoutError("Native-client input deadline exceeded")
        try:
            chunk = os.read(sys.stdin.fileno(), 65536)
        except BlockingIOError:
            pause(0.02)
            continue
        if not chunk:
            return bytes(data)
        data.extend(chunk)
        if len(data) > MAX_INPUT:
            raise ValueError("VM command input exceeds its bound")


def receipt(fd, value):
    import json

    data = (json.dumps(value) + "\n").encode()
    if len(data) > MAX_RECEIPT or os.write(fd, data) != len(data):
        raise OSError("Native-client completion was not fully written")


def main():
    if Path(e2e.__file__).resolve().parents[1] != Path(__file__).resolve().parents[2]:
        raise RuntimeError("VM client selected a different source checkout")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=float, required=True)
    parser.add_argument("--limit", type=int, required=True)
    parser.add_argument("--admission-fd", type=int)
    parser.add_argument("--status-fd", type=int, required=True)
    parser.add_argument("--completion-file", type=Path)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.command[:1] == ["--"]:
        args.command = args.command[1:]
    if not args.command or not 0 < args.timeout <= 86400 or not 1 <= args.limit <= 4 * 1024**3:
        parser.error("Use a command with positive bounded timeout and output limit")
    if args.status_fd < 3 or not stat.S_ISFIFO(os.fstat(args.status_fd).st_mode):
        parser.error("Completion requires an inherited private pipe")
    if args.admission_fd is not None:
        if args.admission_fd < 3 or args.admission_fd == args.status_fd:
            parser.error("Admission requires a distinct non-stdio descriptor")
        os.fstat(args.admission_fd)
    if args.completion_file is not None:
        pending = proof.read(args.completion_file)
        if pending is None or pending["kind"] != "pending":
            parser.error("A native CLI requires its exact pending durable receipt")

    def complete(value):
        # Publish only after canonical process cleanup. A dead reader cannot
        # erase this proof; missing/pending evidence remains a recovery fence.
        proof.finish(args.completion_file, value)
        receipt(args.status_fd, value)

    try:
        body = input_body(args.timeout)
        done = process_run(
            args.command,
            input=body or None,
            timeout=args.timeout,
            stdout=sys.stdout.buffer,
            stderr=sys.stderr.buffer,
            limit=args.limit,
            pass_fds=(),
        )
    except BaseException as error:
        kind = (
            "uncertain"
            if cleanup_uncertain(error)
            else "timeout"
            if isinstance(error, subprocess.TimeoutExpired)
            else "failed"
        )
        event("exception", error=error, partial=True)
        with finalizer(lambda: complete(dict(kind=kind))):
            raise
    complete(dict(kind="completed", returncode=done.returncode))


if __name__ == "__main__":
    raise SystemExit(entry(main, name="verify"))
