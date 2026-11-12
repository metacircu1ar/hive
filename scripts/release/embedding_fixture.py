"""A bounded local HTTP fixture, with no library request threads or shutdown wait."""

from contextlib import contextmanager
import json
import select
import socket
import threading
import time
from .cancellation import cleanup
from .progress import event, finalizer


def receive(client, stop):
    data, expected = bytearray(), None
    deadline = time.monotonic() + 2
    while not stop.is_set() and time.monotonic() < deadline:
        ready, _, _ = select.select([client], [], [], 0.1)
        if not ready:
            continue
        chunk = client.recv(65536)
        if not chunk:
            raise ValueError("Incomplete fixture request")
        data.extend(chunk)
        if expected is None:
            split = data.find(b"\r\n\r\n")
            if split < 0:
                if len(data) > 65536:
                    raise ValueError("Fixture headers exceed their bound")
                continue
            if split > 65536:
                raise ValueError("Fixture headers exceed their bound")
            lines = bytes(data[:split]).split(b"\r\n")
            if not lines[0].startswith(b"POST "):
                raise ValueError("Fixture requires a POST request")
            lengths = [
                line.split(b":", 1)[1].strip()
                for line in lines[1:]
                if line.lower().startswith(b"content-length:")
            ]
            if (
                len(lengths) != 1
                or len(lengths[0]) > 7
                or not lengths[0].isdigit()
                or not 0 < int(lengths[0]) <= 1024 * 1024
            ):
                raise ValueError("Fixture needs one bounded body length")
            expected = split + 4 + int(lengths[0])
        if len(data) > 1024 * 1024 + 65536:
            raise ValueError("Fixture body exceeds its bound")
        if len(data) >= expected:
            return json.loads(data[split + 4 : expected])
    raise TimeoutError("Fixture request exceeded its deadline")


def reply(client, stop, body, status):
    data = (
        f"HTTP/1.1 {status}\r\nContent-Type: application/json\r\n"
        f"Content-Length: {len(body)}\r\nConnection: close\r\n\r\n"
    ).encode() + body
    deadline = time.monotonic() + 2
    while data and not stop.is_set() and time.monotonic() < deadline:
        _, ready, _ = select.select([], [client], [], 0.1)
        if ready:
            count = client.send(data[:65536])
            if not count:
                raise OSError("Fixture connection stopped accepting output")
            data = data[count:]
    if data and not stop.is_set():
        raise TimeoutError("Fixture response exceeded its deadline")


def serve(listener, stop):
    while not stop.is_set():
        try:
            client, _ = listener.accept()
        except TimeoutError:
            continue
        except OSError:
            if not stop.is_set():
                raise
            return
        with client:
            client.setblocking(False)
            try:
                request = receive(client, stop)
                inputs = request["input"]
                if not isinstance(inputs, list) or len(inputs) > 4096:
                    raise ValueError("Fixture input exceeds its bound")
                body = json.dumps(
                    {
                        "data": [
                            dict(index=index, embedding=[1.0, 0.0]) for index in range(len(inputs))
                        ]
                    }
                ).encode()
                reply(client, stop, body, "200 OK")
            except (OSError, ValueError, KeyError, TypeError) as error:
                if not stop.is_set():
                    event("observation_failed", error=error)


@contextmanager
def embedding_server():
    stop = threading.Event()
    errors = []
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(16)
        listener.settimeout(0.1)

        def guarded():
            try:
                serve(listener, stop)
            except BaseException as error:
                errors.append(error)
                stop.set()

        worker = threading.Thread(target=guarded, name="hive-fixture-http", daemon=True)

        def retire():
            with cleanup():
                stop.set()
                deadline = time.monotonic() + 1
                while worker.is_alive() and time.monotonic() < deadline:
                    worker.join(0.1)
                if worker.is_alive():
                    raise RuntimeError("Fixture HTTP thread did not stop before its deadline")
                if errors:
                    raise RuntimeError("Fixture HTTP thread failed") from errors[0]

        with finalizer(retire):
            worker.start()
            yield f"http://127.0.0.1:{listener.getsockname()[1]}/v1/embeddings"
