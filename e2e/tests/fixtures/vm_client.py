"""A native start client deliberately leaves a detached child for recovery."""

import errno
import json
import os
from pathlib import Path
import socket
import shutil
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
import e2e
from e2e.tests.fixtures.campaign_service import frame, request
from release.cancellation import check, pause
from release.files import atomic_write_json
from release.owned_process import spawn
from release.progress import entry


def worker(root):
    # Hold the real child before native PID publication: Lima can still report
    # Stopped during this interval, even after the start CLI is killed.
    expected = json.loads((root / "expected.json").read_text())
    inherited = False
    for fd in range(3, 256):
        try:
            info = os.fstat(fd)
        except OSError as error:
            if error.errno != errno.EBADF:
                raise
        else:
            inherited |= [info.st_dev, info.st_ino] == expected
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
        listener.bind(str(root / "work.sock"))
        listener.listen()
        listener.settimeout(0.1)
        atomic_write_json(
            root / "worker.json", dict(pid=os.getpid(), parent=os.getppid(), inherited=inherited)
        )
        until = time.monotonic() + 30
        while time.monotonic() < until:
            check()
            try:
                stream, _ = listener.accept()
            except TimeoutError:
                continue
            with stream:
                stream.settimeout(2)
                command = frame(stream)
                if command in (["materialize"], ["materialize", "Running"]):
                    row = json.loads((root / "row.json").read_text())
                    row["status"] = "Running" if len(command) == 2 else "Broken"
                    row["hostAgentPID"] = os.getpid()
                    atomic_write_json(root / "row.json", row)
                elif command != ["quit"]:
                    raise ValueError("Unknown fixture workload command")
                stream.sendall(b'{"ok":true}\n')
                if command == ["quit"]:
                    break
    atomic_write_json(root / "worker-done.json", dict(done=True))


def native(root, args):
    if args[0] == "start":
        # This intentionally models Lima's separately grouped hostagent. The
        # fixture client is killed by the test; its child owns its own group.
        with spawn([sys.executable, __file__, str(root), "worker"]) as child:
            until = time.monotonic() + 5
            while not (root / "worker.json").exists():
                if child.exited() or time.monotonic() >= until:
                    raise RuntimeError("Fixture hostagent did not start")
                pause(0.01)
            atomic_write_json(root / "client.json", dict(pid=os.getpid(), custodian=os.getppid()))
            pause(20)
    elif args[0] == "list":
        row = json.loads((root / "row.json").read_text())
        if row is not None:
            print(json.dumps(row))
    elif args[0] == "stop":
        request("unix://" + str(root / "work.sock"), ["quit"])
        until = time.monotonic() + 5
        while not (root / "worker-done.json").exists():
            if time.monotonic() >= until:
                raise TimeoutError("Fixture hostagent did not retire")
            pause(0.01)
        row = json.loads((root / "row.json").read_text())
        row["status"] = "Stopped"
        row.pop("hostAgentPID", None)
        atomic_write_json(root / "row.json", row)
    elif args[0] == "delete":
        row = json.loads((root / "row.json").read_text())
        if row["status"] != "Stopped" or not (root / "worker-done.json").exists():
            raise RuntimeError("Fixture deletion requires the stopped workload")
        shutil.rmtree(row["dir"])
        atomic_write_json(root / "row.json", None)
    else:
        raise ValueError("Unknown fixture native operation")


def main():
    if Path(e2e.__file__).resolve().parents[1] != Path(__file__).resolve().parents[3]:
        raise RuntimeError("Fixture bootstrap selected another checkout")
    root, args = Path(sys.argv[1]), sys.argv[2:]
    worker(root) if args == ["worker"] else native(root, args)


if __name__ == "__main__":
    raise SystemExit(entry(main, name="scenario"))
