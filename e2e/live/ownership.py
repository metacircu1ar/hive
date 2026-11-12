"""One live owner; shared login state is never a disposable fixture."""

from contextlib import contextmanager
import fcntl
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import time

from e2e.campaign import Admission, cleanup_uncertain, recover
from e2e.scripts.harness.cli import Cli, call_deadline, read_agents
from e2e.live.proof import RetirementFailure, Snapshot, verify as verify_retirement
from release.cancellation import check, pause
from release.files import _owned_fd, _private_fd, atomic_write_json, raw_log
from release.owned_process import spawn
from release.process import ProcessFailure, run
from release.progress import finalizer


def default_state():
    return Path(os.environ.get("XDG_STATE_HOME") or Path.home() / ".local/state") / "hive"


def configured_cli(remote, config, *, timeout=45):
    configured = [*remote, "--config", str(config)]
    return Cli(
        remote,
        prefixes={family: configured for family in ("auth", "models", "daemon")},
        timeout=timeout,
    )


def opt_in(parser, args, *, agents, models):
    if not args.live:
        parser.error("Real provider turns require --live and an existing login")
    print(
        json.dumps(
            dict(
                live=True,
                agents=agents,
                work_limit_seconds=args.timeout,
                models=models,
                notice="Uses subscription quota; never logs in or retries inference",
            )
        ),
        flush=True,
    )


@contextmanager
def campaign(args, label):
    admission = Admission()
    admission.recover = lambda active: recover(active, admission=admission)
    with admission.held():
        admission.begin(
            "native",
            label,
            dict(state=str(args.state_dir.resolve()), output=str(args.output_dir.resolve())),
        )
        try:
            yield admission
        except BaseException as error:
            if admission.active["phase"] in ("prepared", "cleaned") and not cleanup_uncertain(
                error
            ):
                with finalizer(admission.complete):
                    raise
            raise
        else:
            admission.complete()


@contextmanager
def stopped_lock(state):
    path = state / "runtime/daemon.lock"
    try:
        fd = _private_fd(path, os.O_RDONLY)
    except FileNotFoundError:
        if (state / "runtime/runtime.sqlite").exists():
            raise ValueError("Populated runtime has no daemon lock") from None
        yield
        return
    with _owned_fd(fd):
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RuntimeError("Stop the existing daemon before live validation") from None
        before, current = os.fstat(fd), path.lstat()
        if (before.st_dev, before.st_ino) != (current.st_dev, current.st_ino):
            raise ValueError("Daemon lock changed during live preflight")
        yield


def paused_snapshot(database):
    database.row_factory = sqlite3.Row
    result = {}
    for row in database.execute("SELECT * FROM comb_collectors WHERE paused=1 ORDER BY name"):
        item = dict(row)
        item.pop("enabled")
        result[row["name"]] = dict(
            binding=item,
            outbox=[
                tuple(r)
                for r in database.execute(
                    "SELECT * FROM comb_outbox WHERE destination=? ORDER BY sequence",
                    (row["name"],),
                )
            ],
        )
    return result


def collector_recovery(state, name, *, binary="hivectl"):
    import uuid

    command = [str(binary), "--state-dir", str(state)]
    return dict(
        prerequisite="After proving owned work stopped, start the daemon for this state using your normal config; inspect before discarding",
        inspect=command + ["collectors", "status"],
        discard_after_inspection=command
        + ["collectors", "discard", name, "--request-id", "e2e-cleanup-" + uuid.uuid4().hex],
    )


def preflight(state, *, collectors=(), binary="hivectl"):
    with stopped_lock(state):
        path = state / "runtime/runtime.sqlite"
        if not path.exists():
            return Snapshot({}, 0, 0)
        with sqlite3.connect(path.as_uri() + "?mode=ro", uri=True, timeout=1) as database:
            count = database.execute(
                "SELECT count(*) FROM agents WHERE "
                "(task_outcome='open' AND expired=0) "
                "OR state NOT IN ('stopped','failed')"
            ).fetchone()[0]
            if count:
                raise RuntimeError("Existing unfinished agents prevent live validation")
            rows = database.execute(
                "SELECT name, paused, pending, gap_count, last_error, "
                "(SELECT count(*) FROM comb_outbox o WHERE o.destination=c.name "
                "AND o.comb_id IS NOT NULL) FROM comb_collectors c ORDER BY name"
            ).fetchall()
            found = [
                dict(destination=n, paused=bool(p), pending=q, gaps=g, last_error=e, uncertain=u)
                for n, p, q, g, e, u in rows
                if q or g or u
            ]
            if found:
                print(
                    json.dumps(
                        dict(
                            pre_launch=True,
                            collectors=found,
                            reserved_recovery={
                                row["destination"]: collector_recovery(
                                    state, row["destination"], binary=binary
                                )
                                for row in found
                                if row["destination"] in collectors
                            },
                            guidance="Inspect collectors status using your normal config; preserve or explicitly discard that evidence",
                        )
                    ),
                    flush=True,
                )
            if any(not row["paused"] or row["destination"] in collectors for row in found):
                raise RuntimeError("Unpaused retained collector work prevents live validation")
            return Snapshot(
                {
                    name: value
                    for name, value in paused_snapshot(database).items()
                    if name not in collectors
                },
                database.execute("SELECT coalesce(max(seq),0) FROM provider_events").fetchone()[0],
                database.execute("SELECT coalesce(max(listing_seq),0) FROM agents").fetchone()[0],
            )


def verify_paused(state, before):
    if not before:
        return
    with (
        stopped_lock(state),
        sqlite3.connect(
            (state / "runtime/runtime.sqlite").as_uri() + "?mode=ro", uri=True, timeout=1
        ) as database,
    ):
        after = paused_snapshot(database)
        if any(after.get(name) != value for name, value in before.items()):
            raise RuntimeError("Live validation changed a pre-existing paused collector")


def terminal(agent):
    return agent["task_outcome"] in ("completed", "cancelled") or agent["expired"]


def owned_agents(call, work, roots=()):
    agents = read_agents(call)
    ids = set(roots) | {a["id"] for a in agents if a["cwd"] == str(work)}
    while True:
        expanded = ids | {a["id"] for a in agents if a["parent_id"] in ids}
        if expanded == ids:
            return [a for a in agents if a["id"] in ids]
        ids = expanded


def cancel_descendants(call, root, discover, timeout=90):
    """Root first, then rediscover until every independent descendant is ended."""
    until = time.monotonic() + timeout

    def terminate(agent):
        need_pause = False

        def pause_open():
            if need_pause:
                call("stop", agent["id"])

        with finalizer(pause_open):
            try:
                call("terminate", agent["id"])
            except (ProcessFailure, OSError, RuntimeError, ValueError, subprocess.SubprocessError):
                need_pause = True

                def observe():
                    nonlocal need_pause
                    need_pause = not terminal(call("status", agent["id"])["agent"])

                with finalizer(observe):
                    raise

    failures = {}
    unknown_native = []

    def preserve_uncertainty():
        if unknown_native:
            first, *later = unknown_native
            first.owned_cleanup_uncertain = True
            first.cleanup_errors = getattr(first, "cleanup_errors", ()) + tuple(later)
            raise first

    with finalizer(preserve_uncertainty), call_deadline(until):
        while True:
            check()
            agents = sorted(
                discover(), key=lambda agent: (agent["parent_id"] is not None, agent["id"] != root)
            )
            if all(terminal(a) and a["state"] in ("stopped", "failed") for a in agents):
                return []
            for agent in agents:
                if terminal(agent):
                    continue
                if time.monotonic() >= until:
                    raise TimeoutError("Hosted-agent cleanup did not finish; retain admission")
                try:
                    terminate(agent)
                except (
                    ProcessFailure,
                    OSError,
                    RuntimeError,
                    ValueError,
                    subprocess.SubprocessError,
                ) as error:
                    failures[agent["id"]] = error
                    if cleanup_uncertain(error) and not any(
                        item is error for item in unknown_native
                    ):
                        unknown_native.append(error)
                else:
                    failures.pop(agent["id"], None)
            if time.monotonic() >= until:
                error = TimeoutError("Hosted-agent cleanup did not finish; retain admission")
                error.cleanup_errors = tuple(failures.values())
                raise error
            pause(min(0.1, max(0, until - time.monotonic())))


class Daemon:
    def __init__(self, call, state, config, output, *, collectors=()):
        self.call, self.state, self.config, self.output = call, state, config, output
        self.process = None
        self.stack = None
        self.snapshot = Snapshot({}, 0, 0)
        self.ready = False
        self.collectors = collectors

    def start(self, admission=None):
        from contextlib import ExitStack

        self.snapshot = preflight(
            self.state,
            collectors=self.collectors,
            binary="hivectl" if self.call is None else self.call.command[0],
        )
        self.stack = ExitStack()
        try:
            log = self.stack.enter_context(raw_log(self.output / "daemon.log"))
            command = [*self.call.command, "--config", str(self.config), "daemon", "run"]
            if admission is not None:
                admission.advance("starting")
            self.process = self.stack.enter_context(
                spawn(command, stdout=log, stderr=log, limit=64 * 1024**2, stop_grace=20)
            )
            until = time.monotonic() + 30
            while True:
                if self.process.exited():
                    raise RuntimeError("Owned daemon exited before readiness")
                done = run(self.call.argv("daemon", "status"), text=True, timeout=5)
                if done.returncode == 0:
                    value = json.loads(done.stdout)
                    if value.get("error") is not None:
                        raise ValueError("Daemon status returned an error with success")
                    if value.get("pid") != self.process.pid:
                        raise RuntimeError("A different daemon won startup; refusing to attach")
                    self.ready = True
                    return value
                failure = json.loads(done.stdout)
                if failure.get("error", {}).get("code") != "daemon_unavailable":
                    raise ProcessFailure(done)
                if time.monotonic() >= until:
                    raise TimeoutError("Owned daemon readiness timed out")
                pause(0.1)
        except BaseException:
            with finalizer(self.close):
                raise

    def close(self):
        def stop():
            if self.process is not None:
                self.process.stop()
                if self.process.returncode != 0:
                    raise RuntimeError(
                        "Owned daemon did not shut down successfully; retain admission"
                    )

        def verify():
            verify_paused(self.state, self.snapshot.paused)
            if self.process is not None:
                with (
                    stopped_lock(self.state),
                    sqlite3.connect(
                        (self.state / "runtime/runtime.sqlite").as_uri() + "?mode=ro",
                        uri=True,
                        timeout=1,
                    ) as database,
                ):
                    try:
                        verify_retirement(database, self.snapshot)
                    except RetirementFailure as error:
                        with finalizer(
                            lambda report=error.report: atomic_write_json(
                                self.output / "retirement-failure.json", report
                            )
                        ):
                            raise

        try:
            with finalizer(verify):
                with finalizer(lambda: self.stack.close() if self.stack is not None else None):
                    stop()
        except BaseException as error:
            error.owned_cleanup_uncertain = True
            raise


def verify_tasks(call):
    if any(
        not terminal(agent) or agent["state"] not in ("stopped", "failed")
        for agent in read_agents(call)
    ):
        raise RuntimeError("Live validation left unfinished agents; retain admission")
