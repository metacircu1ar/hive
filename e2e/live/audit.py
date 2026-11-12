"""Bounded socket audit; bridge credentials never enter the retained records."""

import json
import socket
import socketserver
import sys
import threading
import time

from e2e.campaign import cleanup_uncertain
from e2e.scripts.harness.io import CompleteLines
from e2e.scripts.harness.worker import Worker
from release.cancellation import Cancelled, check
from release.files import append_jsonl_record
from release.progress import cleanup_all, event, finalizer

AUDIT_RECORD_LIMIT = 128 * 1024**2
REQUEST_LIMIT = 8 * 1024**2 + 1
REPLY_LIMIT = 32 * 1024**2 + 1


class FrameError(ValueError):
    pass


def ready(stream, until):
    check()
    left = until - time.monotonic()
    if left <= 0:
        raise TimeoutError("Audit socket deadline exceeded")
    stream.settimeout(min(0.1, left))


def receive(stream, limit, until):
    data = bytearray()
    while True:
        ready(stream, until)
        try:
            block = stream.recv(min(65536, limit + 1 - len(data)))
        except TimeoutError:
            continue
        if not block:
            raise FrameError("Incomplete audit frame")
        data.extend(block)
        if len(data) > limit:
            raise FrameError("Audit frame exceeds its bound")
        if b"\n" in block:
            if data.count(b"\n") != 1 or not data.endswith(b"\n"):
                raise FrameError("Unexpected audit frame boundary")
            return bytes(data)


def send(stream, data, until):
    remaining = memoryview(data)
    while remaining:
        ready(stream, until)
        try:
            count = stream.send(remaining[:65536])
        except TimeoutError:
            continue
        if not count:
            raise FrameError("Audit peer closed during reply")
        remaining = remaining[count:]


class AuditProxy(socketserver.UnixStreamServer):
    request_queue_size = 32
    timeout = 0.1

    def __init__(self, path, upstream, log):
        self.upstream, self.log = str(upstream), log
        self.lock, self.stop_event = threading.Lock(), threading.Event()
        self.workers, self.failures, self.owner = [], [], None
        super().__init__(str(path), AuditHandler, bind_and_activate=False)
        try:
            self.server_bind()
            path.chmod(0o600)
            self.server_activate()
            self.owner = Worker(self.serve, stop=self.stop_event)
            self.owner.start()
        except BaseException:
            with finalizer(self.stop):
                raise

    def serve(self):
        while not self.stop_event.is_set():
            check()
            self.handle_request()

    def handle_error(self, request, address):
        event("observation_failed", error=sys.exc_info()[1], phase="observe", partial=True)
        with self.lock:
            self.failures.append("audit_worker_dispatch_failed")

    def process_request(self, request, address):
        active = []
        for worker in self.workers:
            if worker.is_alive():
                active.append(worker)
            else:
                worker.check()
        self.workers = active
        if len(active) >= 16:
            with self.lock:
                if "audit_concurrency_limit" not in self.failures:
                    self.failures.append("audit_concurrency_limit")
            self.shutdown_request(request)
            return

        def handle():
            with finalizer(lambda: self.shutdown_request(request)):
                self.finish_request(request, address)

        worker = Worker(handle, stop=self.stop_event)
        try:
            worker.start()
        except BaseException:
            with finalizer(lambda: self.shutdown_request(request)):
                raise
        self.workers.append(worker)

    @property
    def errors(self):
        for worker in [self.owner, *self.workers]:
            if worker is not None:
                worker.check()
        with self.lock:
            return list(self.failures)

    def stop(self):
        self.stop_event.set()
        until = time.monotonic() + 15

        def join(worker):
            if worker is None:
                return
            try:
                worker.join(timeout=max(0, until - time.monotonic()))
            except Cancelled as error:
                # The shared parent cancellation was observed and the thread
                # finished. It cannot suppress a real worker cleanup failure.
                if (
                    worker.is_alive()
                    or cleanup_uncertain(error)
                    or getattr(error, "cleanup_errors", ())
                ):
                    raise

        def handlers():
            cleanup_all(*(lambda worker=worker: join(worker) for worker in self.workers))

        with finalizer(self.server_close):
            cleanup_all(lambda: join(self.owner), handlers)
            with self.lock:
                if self.failures:
                    raise RuntimeError("Comb audit retirement found a failed tool exchange")

    def rows(self):
        with self.lock:
            if not self.log.exists():
                return []
            with CompleteLines(self.log, max_record_bytes=AUDIT_RECORD_LIMIT) as stream:
                return [json.loads(line) for line in stream]


class AuditHandler(socketserver.BaseRequestHandler):
    def handle(self):
        began = time.monotonic()
        until = began + 40
        try:
            frame = receive(self.request, REQUEST_LIMIT, until)
            request = json.loads(frame)
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as upstream:
                ready(upstream, until)
                upstream.connect(self.server.upstream)
                send(upstream, frame, until)
                response = receive(upstream, REPLY_LIMIT, until)
            row = dict(
                start=began,
                end=time.monotonic(),
                claim=request["claim"],
                operation=request["operation"],
                reply=json.loads(response),
            )
            if len(json.dumps(row, separators=(",", ":")).encode()) + 1 > AUDIT_RECORD_LIMIT:
                raise FrameError("Audit record exceeds its bound")
            with self.server.lock:
                # Observational evidence is regenerable; the result is accepted
                # only after the complete stream is checked and workers retire.
                append_jsonl_record(
                    self.server.log, row, max_record_bytes=AUDIT_RECORD_LIMIT, durable=False
                )
            send(self.request, response, until)
        except (OSError, ValueError, TypeError, KeyError) as error:
            event("observation_failed", error=error, phase="observe", partial=True)
            with self.server.lock:
                self.server.failures.append(
                    "audit_frame_exceeds_or_invalid"
                    if isinstance(error, FrameError)
                    else "audit_transport_or_protocol_failed"
                )
