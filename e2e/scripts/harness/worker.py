"""Bounded worker ownership with the main thread's canonical cancellation context."""

import threading
from release.cancellation import WorkerStopped, worker_context
from release.native_evidence import detached
from release.progress import event


class Worker:
    def __init__(self, target, *, stop, args=()):
        self.target, self.stop, self.args = target, stop, args
        self.context, self.error = worker_context(stop), None
        self.thread = threading.Thread(target=self._run)

    def _run(self):
        try:
            self.context.run(self._work)
        except BaseException as error:
            if (
                isinstance(error, WorkerStopped)
                and not getattr(error, "cleanup_errors", ())
                and not getattr(error, "owned_cleanup_uncertain", False)
            ):
                return
            self.error = error
            self.stop.set()
            event("observation_failed", error=error, phase="observe", partial=True)

    def _work(self):
        with detached():
            self.target(*self.args)

    def start(self):
        self.thread.start()

    def is_alive(self):
        return self.thread.is_alive()

    def check(self):
        if self.error is not None:
            raise self.error

    def join(self, timeout=15):
        self.thread.join(timeout)
        if self.thread.is_alive():
            error = RuntimeError("Scenario worker cleanup is unconfirmed")
            error.owned_cleanup_uncertain = True
            raise error
        self.check()
