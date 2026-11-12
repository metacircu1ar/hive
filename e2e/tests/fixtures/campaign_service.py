"""A fixture daemon owns work independently of its short-lived control clients."""

import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import socket
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
import e2e
from release.cancellation import check, pause
from release.files import atomic_write_json
from release.owned_process import spawn
from release.progress import entry, event, finalizer


def frame(stream):
    data = bytearray()
    while b"\n" not in data:
        chunk = stream.recv(65536)
        if not chunk or len(data) + len(chunk) > 1024 * 1024:
            raise ValueError("Fixture control frame is incomplete or too large")
        data.extend(chunk)
    if data.count(b"\n") != 1 or not data.endswith(b"\n"):
        raise ValueError("Fixture control has trailing bytes")
    return json.loads(data)


def request(endpoint, args):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(5)
        stream.connect(endpoint.removeprefix("unix://"))
        stream.sendall((json.dumps(args) + "\n").encode())
        return frame(stream)


def client(args):
    reply = request(args.endpoint, args.command)
    if reply.get("block"):
        pause(30)
    sys.stdout.write(reply["stdout"])
    if reply["status"]:
        raise SystemExit(reply["status"])


class Daemon:
    def __init__(self, args, stack):
        self.args, self.stack = args, stack
        self.row, self.worker = None, None
        self.refuse_cleanup = False
        self.stopping = False
        self.defer_create, self.pending = False, None

    def start_worker(self):
        self.worker = self.stack.enter_context(
            spawn(
                [
                    sys.executable,
                    "-c",
                    "import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); time.sleep(30)",
                ]
            )
        )
        self.row["State"]["Running"] = True
        atomic_write_json(self.args.directory / "workload.json", dict(pid=self.worker.pid))

    def command(self, args):
        status, block, body = 0, False, {}
        if args[0] == "info":
            body = dict(ID=self.args.identity)
        elif args[:2] == ["container", "create"]:
            name = args[args.index("--name") + 1]
            label = args[args.index("--label") + 1].split("=", 1)
            self.row = dict(
                Id="a" * 64,
                Name="/" + name,
                Config=dict(Labels={label[0]: label[1]}),
                State=dict(Running=False),
            )
            if self.defer_create:
                self.pending, self.row = self.row, None
                atomic_write_json(self.args.directory / "pending.json", dict(pending=True))
                return dict(status=0, stdout="", block=True)
            return dict(status=0, stdout="a" * 64 + "\n")
        elif args[:2] == ["container", "start"]:
            self.start_worker()
        elif args[:2] == ["container", "ls"]:
            text = "" if self.row is None else json.dumps(dict(ID=self.row["Id"])) + "\n"
            return dict(status=0, stdout=text)
        elif args[:2] == ["container", "inspect"]:
            body = [self.row]
        elif args[:2] == ["container", "exec"]:
            atomic_write_json(self.args.directory / "executing.json", dict(running=True))
            block = True
        elif args[:2] in (["container", "stop"], ["container", "kill"]):
            if self.refuse_cleanup:
                status = 1
            else:
                self.worker.stop()
                self.row["State"]["Running"] = False
        elif args[:2] == ["container", "rm"]:
            if self.row["State"]["Running"]:
                raise ValueError("Fixture workload is still alive")
            self.row = None
            atomic_write_json(self.args.directory / "removed.json", dict(removed=True))
        elif args == ["refuse-cleanup"]:
            self.refuse_cleanup = True
        elif args == ["allow-cleanup"]:
            self.refuse_cleanup = False
        elif args == ["defer-create"]:
            self.defer_create = True
        elif args == ["materialize"]:
            self.row, self.pending = self.pending, None
            self.start_worker()
        elif args == ["quit"]:
            self.stopping = True
        else:
            raise ValueError("Unknown fixture command")
        return dict(status=status, stdout=json.dumps(body) + "\n", block=block)


def daemon(args):
    with ExitStack() as stack, socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
        state = Daemon(args, stack)
        listener.bind(str(args.directory / "control.sock"))
        listener.listen()
        listener.settimeout(0.1)
        atomic_write_json(args.directory / "ready.json", dict(ready=True))
        while not state.stopping:
            check()
            try:
                stream, _ = listener.accept()
            except TimeoutError:
                continue
            with stream:
                stream.settimeout(5)
                reply = state.command(frame(stream))
                try:
                    stream.sendall((json.dumps(reply) + "\n").encode())
                except BrokenPipeError as error:
                    event("observation_failed", error=error, success=False)


def native_child(args):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
        listener.bind(str(args.directory / "native.sock"))
        listener.listen()
        listener.settimeout(0.1)
        atomic_write_json(args.directory / "native-ready.json", dict(pid=os.getpid()))
        until = time.monotonic() + 15
        while time.monotonic() < until:
            check()
            try:
                stream, _ = listener.accept()
            except TimeoutError:
                continue
            with stream:
                stream.settimeout(5)
                if frame(stream) != ["quit"]:
                    raise ValueError("Unknown native fixture command")
                stream.sendall(b'{"stopped":true}\n')
                break
    atomic_write_json(args.directory / "native-done.json", dict(done=True))


def nested_owner(args):
    """Cooperative owner whose delayed cleanup must outlive its shell wrapper."""
    with spawn([sys.executable, __file__, "native-child", str(args.directory)]) as child:
        until = time.monotonic() + 5
        while not (args.directory / "native-ready.json").exists():
            if child.exited() or time.monotonic() >= until:
                raise RuntimeError("Nested fixture child did not start")
            pause(0.01)

        def retire():
            pause(0.7)
            request("unix://" + str(args.directory / "native.sock"), ["quit"])
            child.wait(timeout=2)
            atomic_write_json(args.directory / "owner-done.json", dict(done=True))

        with finalizer(retire):
            atomic_write_json(args.directory / "owner-ready.json", dict(ready=True))
            pause(10)


def main():
    if Path(e2e.__file__).resolve().parents[1] != Path(__file__).resolve().parents[3]:
        raise RuntimeError("Fixture bootstrap selected another checkout")
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="mode", required=True)
    server = commands.add_parser("daemon")
    server.add_argument("directory", type=Path)
    server.add_argument("--identity", default="fixture-daemon")
    child = commands.add_parser("native-child")
    child.add_argument("directory", type=Path)
    nested = commands.add_parser("nested-owner")
    nested.add_argument("directory", type=Path)
    call = commands.add_parser("client")
    call.add_argument("--host", dest="endpoint", required=True)
    call.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    {
        "daemon": daemon,
        "native-child": native_child,
        "nested-owner": nested_owner,
        "client": client,
    }[args.mode](args)


if __name__ == "__main__":
    raise SystemExit(entry(main, name="scenario"))
