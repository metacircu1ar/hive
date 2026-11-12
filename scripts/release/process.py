"""Interpret subprocess outcomes before parsing their structured stdout."""

import json
import subprocess
from .owned_process import spawn
from .progress import event, finalizer
from .native_evidence import current as evidence


def run(
    command,
    *,
    timeout=30,
    input=None,
    text=False,
    check=False,
    env=None,
    cwd=None,
    stdin=None,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    capture_output=False,
    limit=4 * 1024 * 1024,
    stop_grace=0.5,
    pass_fds=(),
):
    if capture_output and (stdout != subprocess.PIPE or stderr != subprocess.PIPE):
        raise ValueError("Capture and explicit output destinations conflict")
    if input is not None and stdin is not None:
        raise ValueError("Input body and explicit input stream conflict")
    body = input.encode() if isinstance(input, str) else input
    source = (
        stdin if stdin is not None else subprocess.PIPE if body is not None else subprocess.DEVNULL
    )
    process, result, sink = None, None, evidence()

    def retain():
        if sink is None or process is None:
            return
        try:
            sink.retain(bytes(process.errors))
        except BaseException as error:
            if result is not None and result.returncode:
                failure = ProcessFailure(result)
                failure.cleanup_errors = (error,)
                raise failure from None
            raise

    with finalizer(retain):
        with spawn(
            command,
            env=env,
            cwd=cwd,
            stdin=source,
            stdout=stdout,
            stderr=stderr,
            limit=limit,
            stop_grace=stop_grace,
            pass_fds=pass_fds,
        ) as process:
            if body is not None:
                process.send(body, close=True)
            status = process.wait(timeout=timeout)
            result = subprocess.CompletedProcess(
                command, status, bytes(process.output), bytes(process.errors)
            )
            if check:
                checked(result)
    if text:
        errors = "strict" if status == 0 else "replace"
        result.stdout = result.stdout.decode("utf-8", errors=errors)
        result.stderr = result.stderr.decode("utf-8", errors=errors)
    return result


class ProcessFailure(RuntimeError):
    def __init__(self, result):
        super().__init__(f"CLI process failed with exit status {result.returncode}")
        # Keep private evidence available to explicit callers, never in log messages.
        self.result = result


def byte_count(value):
    if isinstance(value, bytes):
        return len(value)
    return len(value.encode("utf-8", errors="replace")) if value is not None else 0


def checked(result):
    summary = dict(
        returncode=result.returncode,
        stderr_bytes=byte_count(result.stderr),
        stdout_bytes=byte_count(result.stdout),
    )
    if result.returncode != 0:
        event("subprocess_failed", **summary)
        raise ProcessFailure(result)
    return result


def json_reply(result, *, allow_list=False):
    checked(result)
    summary = dict(
        returncode=result.returncode,
        stderr_bytes=byte_count(result.stderr),
        stdout_bytes=byte_count(result.stdout),
    )
    try:
        value = json.loads(result.stdout)
    except (ValueError, TypeError) as error:
        event("subprocess_invalid_json", error=error, **summary)
        raise RuntimeError("Successful CLI process returned invalid JSON") from error
    if not isinstance(value, dict) and not (allow_list and isinstance(value, list)):
        event("subprocess_invalid_json", **summary)
        raise RuntimeError("Successful CLI process returned an unexpected response shape")
    if isinstance(value, dict) and "error" in value:
        event("subprocess_failed", **summary)
        raise RuntimeError("CLI reported an error despite a successful exit status")
    return value
