"""Owned transport worker; the parent never blocks on network or abandons a writer."""

import hashlib
import json
from pathlib import Path
import sys
from urllib.parse import urlsplit
from urllib.request import urlopen

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from release.files import atomic_stream
from release.progress import entry


def allowed(url, local):
    parsed = urlsplit(url)
    return (
        parsed.scheme == "https"
        or local
        and parsed.scheme == "http"
        and parsed.hostname in {"127.0.0.1", "::1", "localhost"}
    )


def main():
    request = json.loads(sys.stdin.buffer.read(16385))
    if (
        not isinstance(request, dict)
        or set(request) != {"url", "limit", "seconds", "local"}
        or not isinstance(request["url"], str)
        or len(request["url"]) > 8192
        or type(request["local"]) is not bool
        or not allowed(request["url"], request["local"])
        or type(request["limit"]) is not int
        or not 0 < request["limit"] <= 256 * 1024 * 1024
        or not isinstance(request["seconds"], (int, float))
        or not 0 < request["seconds"] <= 60
    ):
        raise ValueError("Invalid bounded download request")

    def consume(output):
        digest, size = hashlib.sha256(), 0
        with urlopen(request["url"], timeout=request["seconds"]) as response:
            if not allowed(response.url, request["local"]):
                raise ValueError("Download redirected outside its admitted transport")
            while True:
                chunk = response.read(min(65536, request["limit"] + 1 - size))
                if not chunk:
                    break
                size += len(chunk)
                if size > request["limit"]:
                    raise ValueError("Download exceeded its byte bound")
                output.write(chunk)
                digest.update(chunk)
        return {"sha256": digest.hexdigest(), "bytes": size}

    receipt = atomic_stream(Path(sys.argv[1]), consume, durable=False, create_once=True)
    print(json.dumps(receipt))


if __name__ == "__main__":
    raise SystemExit(entry(main, name="fetch"))
