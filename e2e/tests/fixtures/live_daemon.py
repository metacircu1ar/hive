"""A native service can fail shutdown or leave a separately grouped child."""

from contextlib import ExitStack
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import sys
import time
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from e2e.tests.fixtures.campaign_service import frame
from release.cancellation import pause
from release.files import atomic_write_json
from release.owned_process import spawn
from release.progress import entry


def provider(root):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
        listener.bind(str(root / "provider.sock"))
        listener.listen()
        listener.settimeout(0.1)
        atomic_write_json(root / "provider-ready.json", dict(pid=os.getpid()))
        until = time.monotonic() + 30
        while time.monotonic() < until:
            try:
                stream, _ = listener.accept()
            except TimeoutError:
                continue
            with stream:
                stream.settimeout(1)
                command = frame(stream)
                if command not in (["ping"], ["quit"]):
                    raise ValueError("Unexpected fixture provider command")
                stream.sendall(b'{"ok":true}\n')
                if command == ["quit"]:
                    break
    atomic_write_json(root / "provider-done.json", dict(done=True))


def service(root):
    mode_path = root / "mode"
    mode = mode_path.read_text() if mode_path.exists() else "normal"
    stopped = [False]
    signal.signal(
        signal.SIGTERM,
        signal.SIG_IGN if mode == "ignore-child" else lambda *_: stopped.__setitem__(0, True),
    )
    with ExitStack() as stack:
        if mode == "ignore-child":
            child = stack.enter_context(spawn([sys.executable, __file__, str(root), "provider"]))
            until = time.monotonic() + 5
            while not (root / "provider-ready.json").exists():
                if child.exited() or time.monotonic() >= until:
                    raise RuntimeError("Fixture provider did not reach readiness")
                pause(0.01)
            with sqlite3.connect(root / "runtime/runtime.sqlite") as database:
                database.execute(
                    "INSERT INTO provider_events(provider,at,kind,data) VALUES(?,0,'started','{}')",
                    (uuid.uuid4().hex,),
                )
        atomic_write_json(root / "ready.json", dict(pid=os.getpid()))
        until = time.monotonic() + 15
        while not stopped[0] and time.monotonic() < until:
            pause(0.01)
        if mode == "late":
            with sqlite3.connect(root / "runtime/runtime.sqlite") as database:
                database.execute(
                    "INSERT INTO agents(id,data,state,task_outcome,expired,thread_id,listing_seq) VALUES('late',?,'stopped','open',0,NULL,1)",
                    (json.dumps(dict(conversation_clean=False)),),
                )
        if mode == "fail":
            raise SystemExit(2)


def main():
    root = Path(sys.argv[1])
    if sys.argv[2:3] == ["provider"]:
        provider(root)
    else:
        service(root)


if __name__ == "__main__":
    raise SystemExit(entry(main, name="scenario"))
