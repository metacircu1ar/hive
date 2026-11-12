"""Bounded, cooperative hashing supported by Python 3.10 and newer."""

import hashlib
from release.cancellation import check


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while True:
            check()
            chunk = stream.read(1024 * 1024)
            if not chunk:
                return digest.hexdigest()
            digest.update(chunk)
