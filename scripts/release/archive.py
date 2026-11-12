"""Stream pinned files into an archive and verify the bytes actually consumed."""

import hashlib
import io
import json
from pathlib import Path
import tarfile


class Payload:
    def __init__(self, source, mode, expected=None):
        self.source, self.mode = source, mode
        self.record = (
            expected
            if expected is not None
            else {"sha256": hashlib.sha256(source).hexdigest(), "bytes": len(source)}
        )

    def manifest(self):
        return {key: self.record[key] for key in ("sha256", "bytes")} | {"mode": self.mode}


class VerifiedReader:
    def __init__(self, stream):
        self.stream, self.digest, self.size = stream, hashlib.sha256(), 0

    def read(self, size=-1):
        data = self.stream.read(size)
        self.digest.update(data)
        self.size += len(data)
        return data

    def finish(self, expected):
        if (
            self.read(1)
            or self.size != expected["bytes"]
            or self.digest.hexdigest() != expected["sha256"]
        ):
            raise ValueError("Consumed frozen file identity mismatch")


class RecordedWriter:
    def __init__(self, output):
        self.output, self.digest, self.size = output, hashlib.sha256(), 0

    def write(self, data):
        count = self.output.write(data)
        if count != len(data):
            raise OSError("Incomplete release archive write")
        self.digest.update(data)
        self.size += count
        return count

    def flush(self):
        self.output.flush()


def write_archive(output, payloads):
    recorded = RecordedWriter(output)
    with tarfile.open(fileobj=recorded, mode="w:gz", format=tarfile.USTAR_FORMAT) as archive:
        for name, payload in sorted(payloads.items()):
            info = tarfile.TarInfo(name)
            info.size, info.mode, info.mtime = payload.record["bytes"], payload.mode, 0
            with (
                io.BytesIO(payload.source)
                if isinstance(payload.source, bytes)
                else Path(payload.source).open("rb")
            ) as source:
                reader = VerifiedReader(source)
                archive.addfile(info, reader)
                reader.finish(payload.record)
    return {"sha256": recorded.digest.hexdigest(), "bytes": recorded.size}


def json_bytes(value):
    return (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()
