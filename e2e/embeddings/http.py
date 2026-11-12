"""Bounded local embedding POSTs in an owned, cancellable worker process."""

import json
import errno
import sys
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

from release.process import json_reply, run
from release.progress import entry
from e2e.paths import ROOT

RESPONSE_BYTES = 16 * 1024**2
REQUEST_BYTES = 1024**2


class HTTPFailure(RuntimeError):
    def __init__(self, status, body):
        super().__init__("Local embedding request was rejected")
        self.status, self.body = status, body


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, newurl):
        return None


def post(url, payload, timeout=120):
    reply = json_reply(
        run(
            [sys.executable, "-m", "e2e.embeddings.http"],
            input=json.dumps(dict(url=url, payload=payload, timeout=timeout)),
            text=True,
            timeout=timeout + 5,
            limit=2 * RESPONSE_BYTES,
            cwd=ROOT,
        )
    )
    if reply["status"] != 200:
        raise HTTPFailure(reply["status"], reply["body"])
    return reply["body"]


def consume(response):
    data = response.read(RESPONSE_BYTES + 1)
    if len(data) > RESPONSE_BYTES:
        raise ValueError("Embedding response exceeded its byte bound")
    return json.loads(data)


def worker():
    data = sys.stdin.buffer.read(REQUEST_BYTES + 1)
    if len(data) > REQUEST_BYTES:
        raise ValueError("Embedding request exceeded its byte bound")
    request = json.loads(data)
    url = urlsplit(request["url"])
    if (
        url.scheme != "http"
        or url.hostname not in ("127.0.0.1", "localhost", "::1")
        or url.username is not None
        or url.password is not None
        or type(request["timeout"]) not in (int, float)
        or not 0 < request["timeout"] <= 120
    ):
        raise ValueError("Only bounded loopback HTTP embedding requests are supported")
    body = None if request["payload"] is None else json.dumps(request["payload"]).encode()
    try:
        response = build_opener(ProxyHandler({}), NoRedirect()).open(
            Request(request["url"], body, {"Content-Type": "application/json"}),
            timeout=request["timeout"],
        )
    except HTTPError as error:
        with error:
            result = dict(status=error.code, body=consume(error))
    except URLError as error:
        cause = error.reason
        if request["payload"] is not None or not (
            isinstance(cause, TimeoutError)
            or isinstance(cause, OSError)
            and cause.errno in (errno.ECONNREFUSED, errno.ECONNRESET, errno.ETIMEDOUT)
        ):
            raise
        result = dict(status=0, body=dict(code="not_ready", errno=getattr(cause, "errno", None)))
    else:
        with response:
            result = dict(status=response.status, body=consume(response))
    print(json.dumps(result))


if __name__ == "__main__":
    raise SystemExit(entry(worker, name="verify"))
