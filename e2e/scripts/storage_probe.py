"""Owned child used by the alternating-build comparison's storage observer."""

import argparse
import fcntl
import os
import sys
import time

from release.cancellation import pause
from release.files import AppendStream
from release.progress import entry, finalizer

RECORD_BYTES = 16 * 1024
FULL_FSYNC = 51


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory")
    parser.add_argument("interval", type=float)
    args = parser.parse_args()
    path = os.path.join(args.directory, "record")
    fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o600)
    with finalizer(lambda: os.close(fd)):
        with AppendStream.create_new(
            os.path.join(args.directory, "samples.jsonl"), durable=False
        ) as stream:
            print("ready", flush=True)
            record = os.urandom(RECORD_BYTES)
            while True:
                pause(args.interval)
                began = time.monotonic_ns()
                written = os.pwrite(fd, record, 0)
                if written != len(record):
                    raise OSError("Incomplete storage-probe record write")
                if sys.platform == "darwin":
                    fcntl.fcntl(fd, FULL_FSYNC)
                else:
                    os.fsync(fd)
                stream.append_jsonl(
                    dict(monotonic_ns=began, sync_ms=(time.monotonic_ns() - began) / 1e6)
                )


if __name__ == "__main__":
    raise SystemExit(entry(main, name="observe"))
