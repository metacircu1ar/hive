"""One bounded JSON-line exchange with cancellable nonblocking Unix-socket IO."""

import errno
import json
import os
import select
import socket
import time
from .cancellation import bounded, check


def exchange(address, request):
    body = json.dumps(request).encode() + b"\n"
    if len(body) > 1024 * 1024:
        raise ValueError("Corpus request exceeds its bound")
    until = time.monotonic() + bounded(5)
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.setblocking(False)
        status = stream.connect_ex(str(address))
        pending = {errno.EINPROGRESS, errno.EAGAIN, errno.EWOULDBLOCK, errno.EALREADY}
        while status:
            if status not in pending:
                raise OSError(status, os.strerror(status))
            wait(stream, until, writing=True)
            status = stream.getsockopt(socket.SOL_SOCKET, socket.SO_ERROR)
        while body:
            wait(stream, until, writing=True)
            count = stream.send(body[:65536])
            if not count:
                raise RuntimeError("Corpus peer closed before accepting its request")
            body = body[count:]
        reply = bytearray()
        while b"\n" not in reply:
            wait(stream, until)
            data = stream.recv(min(65536, 1024 * 1024 + 1 - len(reply)))
            if not data:
                raise RuntimeError("Corpus peer closed before a complete reply")
            reply.extend(data)
            if len(reply) > 1024 * 1024:
                raise ValueError("Corpus reply exceeded its bound")
        return json.loads(reply[: reply.index(b"\n")])


def wait(stream, until, *, writing=False):
    while True:
        check()
        remaining = until - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("Corpus exchange exceeded its deadline")
        ready = select.select(
            [] if writing else [stream], [stream] if writing else [], [], min(0.1, remaining)
        )
        if ready[1 if writing else 0]:
            return
