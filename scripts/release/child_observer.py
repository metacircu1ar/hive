"""Observe owned child exit without reaping its process-group identity pin."""

import errno
import ctypes
import os
import select
import sys
from .cancellation import check


class OwnershipLost(RuntimeError):
    pass


def only_leader(pid):
    """Bounded Darwin membership proof; caller separately proves leader exit."""
    if sys.platform != "darwin":
        return False
    library = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    query = library.proc_listpgrppids
    query.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_int]
    query.restype = ctypes.c_int
    entries = (ctypes.c_int * 2)()
    ctypes.set_errno(0)
    count = query(pid, entries, ctypes.sizeof(entries))
    if count < 0 or count == 0 and ctypes.get_errno():
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code))
    return count == 0 or count == 1 and entries[0] == pid


class Observer:
    def __init__(self):
        self.pid, self.queue, self.exited, self.lost = None, None, False, False
        if sys.platform == "darwin":
            self.queue = select.kqueue()
        elif not sys.platform.startswith("linux"):
            raise RuntimeError("Owned child observation requires macOS or Linux")

    def attach(self, pid):
        self.pid = pid
        if self.queue is not None:
            try:
                event = select.kevent(
                    pid,
                    filter=select.KQ_FILTER_PROC,
                    flags=select.KQ_EV_ADD | select.KQ_EV_ONESHOT,
                    fflags=select.KQ_NOTE_EXIT,
                )
                self.queue.control([event], 0, 0)
            except OSError as error:
                if error.errno == errno.ESRCH:
                    self.exited = True
                else:
                    raise

    def ready(self, seconds=0):
        check()
        if self.lost:
            raise OwnershipLost("Owned child status was lost; signals are forbidden")
        status = self.owned()
        if self.exited:
            return True
        if self.queue is not None:
            events = self.queue.control(None, 1, min(0.1, seconds))
            if events:
                event = events[0]
                if event.flags & select.KQ_EV_ERROR:
                    raise OSError(event.data, os.strerror(event.data))
                if (
                    event.ident != self.pid
                    or event.filter != select.KQ_FILTER_PROC
                    or not event.fflags & select.KQ_NOTE_EXIT
                ):
                    raise RuntimeError("Unexpected native child observation")
                self.exited = True
        else:
            if status is not None:
                if status.si_pid != self.pid or status.si_code not in {
                    os.CLD_EXITED,
                    os.CLD_KILLED,
                    os.CLD_DUMPED,
                }:
                    raise RuntimeError("Unexpected native child wait status")
                self.exited = True
        return self.exited

    def owned(self):
        try:
            if self.queue is None:
                return os.waitid(os.P_PID, self.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
            # Ownership probe only; kqueue determines readiness. XNU waitid's exit
            # category is not used. Native flags from sys/wait.h, available on 11.0.
            library = ctypes.CDLL(None, use_errno=True)
            library.waitid.argtypes = [ctypes.c_int, ctypes.c_uint, ctypes.c_void_p, ctypes.c_int]
            library.waitid.restype = ctypes.c_int
            status = (ctypes.c_long * 64)()
            for _ in range(16):
                check()
                if library.waitid(1, self.pid, ctypes.byref(status), 0x01 | 0x04 | 0x20) == 0:
                    return None
                code = ctypes.get_errno()
                if code != errno.EINTR:
                    raise OSError(code, os.strerror(code))
            raise InterruptedError(errno.EINTR, "Owned status probe was repeatedly interrupted")
        except OSError as error:
            if error.errno != errno.ECHILD:
                raise
            self.lost = True
            raise OwnershipLost("Owned child status was lost; signals are forbidden") from error

    def close(self):
        if self.queue is not None:
            self.queue.close()
