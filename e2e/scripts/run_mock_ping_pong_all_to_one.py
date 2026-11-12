from contextlib import ExitStack
from release.owned_process import spawn
from release.files import create_directory

"""Run the deterministic ping pong all to one test through the real Hive runtime, without Codex.

--agents counts pong plus its children. Only the initial root spawn is driven by
this script; all subsequent coordination comes from the mock provider's tools.
"""
from release.cancellation import check, finish, pause
from e2e.scripts.harness.audit import cancellable

#!/usr/bin/env python3
from release.files import atomic_copy, atomic_write_text

import argparse
import collections
import json
from release.files import raw_log, AppendStream, append_jsonl_record
from e2e.scripts.harness.io import json_lines
import os
import shutil
import sqlite3
import subprocess
import tempfile
import time
import uuid
from contextlib import closing
from functools import partial
from pathlib import Path

from e2e.paths import product_default, product_root, require_paths

from e2e.scripts.harness.checks import one as require_one
from e2e.scripts.harness.checks import require
from e2e.scripts.harness.cli import Cli
from release.progress import cleanup_all, finalizer
from release.acceptance import mock_login
from e2e.scripts.harness.evidence import index_events
from e2e.scripts.harness.traces import MemoryTraceEvidence, TraceVerifier
from e2e.scripts.scenario_memory import Sampler, process_table


def remaining_owned(sampler, timeout=10):
    """Observe original descendants by PID+birth time; never signal a reused PID."""
    deadline = time.monotonic() + timeout
    while True:
        table = process_table()
        remaining = [
            row for row in table.values() if (row["pid"], row["start_time"]) in sampler.owned
        ]
        if not remaining or time.monotonic() >= deadline:
            return remaining
        pause(0.1)


def verify_database(path, root, count, *, require_recycling=True):
    """Independent, indexed verification from a consistent runtime snapshot."""
    with closing(sqlite3.connect(f"file:{path}?mode=ro", uri=True)) as db, cancellable(db):
        return _verify_database(db, root, count, require_recycling=require_recycling)


def _verify_database(db, root, count, *, require_recycling=True):
    return AllToOneAudit(db, root, count, require_recycling).verify()


class AllToOneAudit:
    """Indexed evidence for the bounded all-to-one graph."""

    def __init__(self, db, root, count, require_recycling):
        self.db, self.root, self.count = db, root, count
        self.require_recycling = require_recycling

    def verify(self):
        self.load_events()
        self.verify_deliveries()
        self.verify_providers()
        self.native = set()
        self.resumed = set()
        self.generations = {}
        self.sessions = []
        self.receipts, self.cancellations = [], []
        self.root_seed = self.one(
            [m for m in self.messages.values() if m["sender"] == "human"], "root seed"
        )
        require(self.root_seed["recipient"] == self.root, "Human task sent to a child")
        self.expected = set()
        for aid, agent in self.agents.items():
            check()
            self.agent(aid, agent)
        self.completion()
        return self.result()

    def load_events(self):
        self.db.row_factory = sqlite3.Row
        records = "agent_records"
        self.agents = {
            r["id"]: json.loads(r["data"])
            for r in self.db.execute(f"SELECT id,data FROM {records}")
        }
        require(
            len(self.agents) == self.count and self.root in self.agents, "Wrong agent count/root"
        )
        self.children = set(self.agents) - {self.root}
        wanted = (
            "spawned",
            "session_ready",
            "execution_failed",
            "model_rerouted",
            "session_quarantined",
            "termination_requested",
            "completion_requested",
            "task_completed",
            "turn_started",
            "turn_finished",
            "message_accepted",
            "provider_started",
            "provider_session_released",
            "provider_shutdown",
        )
        query = (
            "SELECT seq,agent,execution,kind,data FROM events WHERE kind IN ("
            + ",".join("?" for _ in wanted)
            + ") ORDER BY seq"
        )
        self.events = index_events(self.db.execute(query, wanted), by_agent=True)
        self.messages = {r["id"]: dict(r) for r in self.db.execute("SELECT * FROM messages")}
        require(len(self.messages) == 4 * self.count - 3, "Wrong exact message count")
        self.by_exchange = collections.defaultdict(list)
        for m in self.messages.values():
            self.by_exchange[m["sender"], m["recipient"], m["body"]].append(m)

    def verify_deliveries(self):
        self.starts, self.finishes, self.accepted = {}, {}, {}
        for aid in self.agents:
            for event in self.events[aid]["turn_started"]:
                mid = event["data"]["message_id"]
                require(mid not in self.starts, "Message delivered more than once")
                self.starts[mid] = (aid, event)
            for event in self.events[aid]["turn_finished"]:
                mid = event["data"]["message_id"]
                require(mid not in self.finishes, "Duplicate turn finish")
                self.finishes[mid] = (aid, event)
            for event in self.events[aid]["message_accepted"]:
                mid = event["data"]["id"]
                require(mid not in self.accepted, "Duplicate message acceptance")
                self.accepted[mid] = (aid, event)
        require(
            set(self.starts) == set(self.finishes) == set(self.messages),
            "Missing or extra turn deliveries",
        )
        for mid, m in self.messages.items():
            aid, start = self.starts[mid]
            end_aid, end = self.finishes[mid]
            require(
                start["data"]["model"] == self.agents[aid]["model"]
                and start["data"]["effort"] == self.agents[aid]["effort"],
                "Turn audit changed model/effort",
            )
            require(
                aid == end_aid == m["recipient"]
                and start["seq"] < end["seq"]
                and start["data"]["turn_id"] == end["data"]["turn_id"]
                and m["state"] == end["data"]["status"],
                "Invalid delivery lifecycle",
            )

    def verify_providers(self):
        self.provider_starts, self.provider_ends = {}, {}
        for row in self.db.execute("SELECT provider,kind,data FROM provider_events ORDER BY seq"):
            data = json.loads(row["data"])
            if row["kind"] == "started":
                require(row["provider"] not in self.provider_starts, "Duplicate provider start")
                self.provider_starts[row["provider"]] = data
            if row["kind"] == "shutdown":
                require(row["provider"] not in self.provider_ends, "Duplicate provider shutdown")
                require(
                    data["shutdown"]["confirmed"]
                    and data["shutdown"]["quiescent"]
                    and data["shutdown"]["exit_code"] == 0,
                    "Unconfirmed provider shutdown",
                )
                self.provider_ends[row["provider"]] = data
        require(
            self.provider_starts and self.provider_starts.keys() == self.provider_ends.keys(),
            "Leaked provider generations",
        )

    one = staticmethod(partial(require_one, qualifier="exactly one"))

    def message(self, sender, recipient, body):
        return self.one(self.by_exchange[sender, recipient, body], body)

    def sent_from(self, sent, trigger):
        recipient, acceptance = self.accepted[sent["id"]]
        sender, source = self.starts[trigger["id"]]
        require(
            recipient == sent["recipient"]
            and sender == sent["sender"]
            and acceptance["data"]["sender"] == sender
            and acceptance["data"]["sender_turn_id"] == source["data"]["turn_id"]
            and source["seq"] < acceptance["seq"] < self.finishes[trigger["id"]][1]["seq"],
            "Message was not sent from the expected input turn",
        )

    def agent(self, aid, a):
        require(
            a["state"] == "stopped"
            and a["turn_id"] is None
            and a["control"] is None
            and a["error"] is None
            and not a["expired"],
            f"{aid}: not cleanly stopped",
        )
        require(a["model"] == "gpt-5.6-luna" and a["effort"] == "low", "Wrong model/effort")
        require(
            not any(
                self.events[aid][k]
                for k in ("execution_failed", "model_rerouted", "session_quarantined")
            ),
            "Execution failure was hidden",
        )
        ready = self.events[aid]["session_ready"]
        require(ready and a["thread_id"] not in self.native, "Missing/shared native conversation")
        self.native.add(a["thread_id"])
        require(
            all(
                e["data"]["thread_id"] == a["thread_id"]
                and e["data"]["model"] == a["model"]
                and e["data"]["effort"] == a["effort"]
                for e in ready
            ),
            "Conversation/model changed",
        )
        self.sessions_ready(aid, a, ready)
        spawned = self.one(self.events[aid]["spawned"], "spawn")
        startup = self.messages[spawned["data"]["initial_message_id"]]
        self.expected.add(startup["id"])
        require(startup["state"] == "completed", "Startup did not complete")
        if aid == self.root:
            require(
                a["parent_id"] is None
                and a["role"] == "pong"
                and a["task_outcome"] == "completed"
                and startup["sender"] == "human",
                "Invalid pong root",
            )
            return
        self.child(aid, a, spawned, startup)

    def sessions_ready(self, aid, a, ready):
        generation_by_exec = {}
        for e in self.events[aid]["provider_started"]:
            generation = e["data"]["provider_id"]
            require(generation in self.provider_ends, "Execution lacks physical cleanup")
            generation_by_exec[e["execution"]] = generation
            self.generations[e["execution"]] = generation
        self.sessions.extend(
            (generation_by_exec[e["execution"]], a["thread_id"], e["data"]["resumed"])
            for e in ready
        )
        if len({generation_by_exec[e["execution"]] for e in ready}) > 1:
            require(any(e["data"]["resumed"] for e in ready), "Recycling created a fresh session")
            self.resumed.add(aid)

    def child(self, aid, a, spawned, startup):
        require(
            a["parent_id"] == self.root
            and a["role"] == "ping"
            and a["lifetime"] == "independent"
            and a["task_outcome"] == "cancelled"
            and startup["sender"] == self.root,
            "Child was not created/terminated by pong",
        )
        require(
            spawned["data"]["parent_id"] == self.root
            and startup["recipient"] == aid
            and startup["body"] == "Send ping to your parent now."
            and self.starts[self.root_seed["id"]][1]["seq"]
            < spawned["seq"]
            < self.finishes[self.root_seed["id"]][1]["seq"],
            "Child was not spawned in the root initial turn",
        )
        require(
            not self.events[aid]["completion_requested"] and not self.events[aid]["task_completed"],
            "Child self-completed",
        )
        cancelled = self.one(self.events[aid]["termination_requested"], "child termination")
        require(cancelled["data"]["by"] == self.root, "Wrong cancellation actor")
        ping = self.message(aid, self.root, "ping")
        pong = self.message(self.root, aid, "pong")
        receipt = self.message(aid, self.root, "received pong")
        for sent, trigger in ((ping, startup), (pong, ping), (receipt, pong)):
            self.sent_from(sent, trigger)
            self.expected.add(sent["id"])
        require(ping["state"] == receipt["state"] == "completed", "Undelivered ping/receipt")
        require(pong["state"] in ("completed", "interrupted"), "Undelivered pong")
        if pong["state"] == "interrupted":
            require(
                self.accepted[receipt["id"]][1]["seq"]
                < cancelled["seq"]
                < self.finishes[pong["id"]][1]["seq"],
                "Pong interrupted without receipt and cancellation",
            )
        self.receipts.append(self.starts[receipt["id"]][1]["seq"])
        self.cancellations.append(cancelled["seq"])

    def completion(self):
        require(self.expected == set(self.messages), "Unexpected messages")
        complete = self.one(
            self.events[self.root]["completion_requested"], "root completion request"
        )
        completed = self.one(self.events[self.root]["task_completed"], "root completion")
        require(
            complete["data"]["by"] == self.root
            and max(self.receipts) < min(self.cancellations)
            and max(self.cancellations) < complete["seq"] < completed["seq"],
            "Completion/cancellation order is wrong",
        )
        if self.require_recycling:
            require(
                self.resumed, "Test did not exercise same-thread resume after process recycling"
            )
        require(
            self.db.execute("SELECT active FROM runtime_counts").fetchone()[0] == 0,
            "Active capacity leaked",
        )

    def result(self):
        manifest = dict(
            providers=self.provider_starts,
            sessions=self.sessions,
            threads={a["thread_id"]: aid for aid, a in self.agents.items()},
            turns={},
        )
        for mid, (aid, event) in self.starts.items():
            turn = event["data"]["turn_id"]
            require(turn not in manifest["turns"], "Native turn identity reused")
            manifest["turns"][turn] = dict(
                message=mid,
                thread=self.agents[aid]["thread_id"],
                provider=self.generations[event["execution"]],
                status=self.finishes[mid][1]["data"]["status"],
            )
        first_pong = self.db.execute(
            "SELECT min(e.at)-(SELECT min(at) FROM events WHERE agent=? AND kind='spawned') FROM events e JOIN messages m ON m.id=json_extract(e.data,'$.id') WHERE e.kind='message_accepted' AND m.sender=? AND m.body='pong'",
            (self.root, self.root),
        ).fetchone()[0]
        return dict(
            _manifest=manifest,
            first_pong_seconds=first_pong,
            agents=self.count,
            ping_agents=len(self.children),
            messages=len(self.messages),
            turns=len(self.starts),
            pings=len(self.children),
            pongs=len(self.children),
            receipts=len(self.children),
            explicit_cancellations=len(self.children),
            native_conversations=len(self.native),
            resumed_agents=len(self.resumed),
            confirmed_provider_shutdowns=len(self.provider_ends),
        )


def verify_traces(paths, count, limits, manifest):
    return TraceVerifier(MemoryTraceEvidence(manifest, count), limits).verify(paths)


PRESETS = {
    "eviction": dict(max_loaded=64, max_providers=4, max_active_turns=64, max_starting=16),
    "high-residency": dict(
        max_loaded=1024, max_providers=16, max_active_turns=256, max_starting=32
    ),
}


def runtime_limits(args):
    selected = {
        key: getattr(args, key) if getattr(args, key) is not None else value
        for key, value in PRESETS[args.preset].items()
    }
    require(selected["max_loaded"] >= 8, "--max-loaded must be >=8")
    require(selected["max_providers"] > 0, "--max-providers must be positive")
    require(selected["max_starting"] > 0, "--max-starting must be positive")
    require(
        selected["max_active_turns"] >= 2,
        "--max-active-turns must be >=2 to verify overlapping turns",
    )
    # Eviction runs deliberately exceed capacity. High-residency runs may retain
    # the whole graph; never force an unload just to satisfy the old verifier.
    loaded = min(selected["max_loaded"], args.agents - (args.preset == "eviction"))
    providers = min(selected["max_providers"], loaded)
    sessions = (loaded + providers - 1) // providers
    require(
        2 <= sessions <= 1024,
        "Derived sessions_per_codex_process must be 2..1024; adjust --max-loaded or --max-providers "
        "(at least two sessions are needed to verify same-process overlap)",
    )
    require(
        0 <= args.warm_providers <= providers,
        "--warm-providers must be between 0 and the effective provider limit",
    )
    return dict(
        max_agents=args.agents,
        max_loaded=loaded,
        max_providers=providers,
        sessions_per_codex_process=sessions,
        max_starting=min(selected["max_starting"], loaded),
        max_active_turns=min(selected["max_active_turns"], loaded),
        idle_seconds=0,
        cohort_min_seconds=1,
        warm_providers=args.warm_providers,
        warm_idle_seconds=60,
    )


def run(args):
    scenario = AllToOneRun(args).run()
    check()
    return scenario.report()


class AllToOneRun:
    """Own one bounded graph run, its observers, and retained evidence."""

    @property
    def recycling_required(self):
        return self.args.preset == "eviction" and self.limits["max_providers"] == 1

    def __init__(self, args):
        self.args = args
        self.project = Path(__file__).resolve().parent.parent
        self.limits = runtime_limits(self.args)
        self.output = self.args.output_dir.resolve()
        create_directory(self.output)
        self.args.output_owned = True
        self.temporary = Path(tempfile.mkdtemp(prefix="hl-scale-", dir="/tmp"))
        self.state = self.temporary / "state"
        self.config = self.temporary / "config.yaml"
        self.prepare_config()
        self.command = [
            str(self.args.binary.resolve()),
            "--config",
            str(self.config),
            "--state-dir",
            str(self.state),
            "--codex-bin",
            str(self.provider),
        ]

        remote = [self.command[0], "--state-dir", str(self.state)]
        control = remote + ["--config", str(self.config)]
        self.call = Cli(remote, prefixes={"daemon": control}, timeout=60)

        self.daemon = self.sampler = self.database = None
        self.root = None
        self.success = False
        self.began = time.monotonic()

    def prepare_config(self):
        # JSON is a YAML subset; no third-party Python package is needed.
        configuration = dict(
            auth_profiles={"mock": {"provider": "codex", "method": "subscription"}},
            runtime=self.limits,
            roles={},
        )
        for role in ("pong", "ping"):
            instructions = self.temporary / f"{role}.md"
            atomic_write_text(instructions, "Deterministic mock ping-pong role.\n")
            configuration["roles"][role] = dict(
                auth="mock",
                model="gpt-5.6-luna",
                effort="low",
                instructions=str(instructions),
                lifetime="independent",
                sandbox="read-only",
            )
        atomic_write_text(self.config, json.dumps(configuration, indent=2) + "\n")
        atomic_copy(self.config, self.output / "config.json")
        self.provider = self.temporary / "mock-provider"
        atomic_copy(self.project / "tests/fixtures/scale_provider.py", self.provider, mode=0o700)
        atomic_copy(
            product_root() / "tests/fixtures/mock_common.py", self.temporary / "mock_common.py"
        )
        atomic_copy(
            product_root() / "tests/fixtures/codex-schema.json",
            self.temporary / "codex-schema.json",
        )
        atomic_copy(self.provider, self.output / "mock-provider.py")
        atomic_copy(self.temporary / "mock_common.py", self.output / "mock_common.py")
        atomic_copy(self.temporary / "codex-schema.json", self.output / "codex-schema.json")
        atomic_copy(product_root() / "scripts/release/files.py", self.temporary / "mock_files.py")
        atomic_copy(self.temporary / "mock_files.py", self.output / "mock_files.py")

    def run(self):
        with ExitStack() as resources:
            self.resources = resources
            with finalizer(self.cleanup):
                self.start()
                self.wait_for_completion()
                self.shutdown()
                self.verify()
        self.prepare_report()
        return self

    def start(self):
        mock_login(self.command)
        log = self.resources.enter_context(raw_log(self.output / "daemon.log"))
        self.daemon = self.resources.enter_context(
            spawn(
                self.command + ["daemon", "run"],
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=log,
                limit=64 * 1024 * 1024,
            )
        )
        for _ in range(100):
            require(not self.daemon.exited(), "Daemon exited during startup")
            try:
                status = self.call("daemon", "status")
                break
            except (RuntimeError, json.JSONDecodeError):
                pause(0.05)
        else:
            raise RuntimeError("Daemon startup timed out")
        self.daemon_ready_ns = time.monotonic_ns()
        self.sampler = Sampler(status["pid"], self.output, max_rss_gib=8)
        self.sampler.start()
        body = self.temporary / "task.json"
        atomic_write_text(body, json.dumps({"children": self.args.agents - 1}))
        self.root = self.call(
            "spawn", "pong", "--cwd", str(self.temporary), "--body-file", str(body)
        )["agent_id"]
        atomic_write_text(
            self.output / "checkpoint.json",
            json.dumps(
                dict(root_id=self.root, command=self.command, state_dir=str(self.state)), indent=2
            )
            + "\n",
        )
        self.database = sqlite3.connect(
            f"file:{self.state}/runtime/runtime.sqlite?mode=ro", uri=True
        )

    def wait_for_completion(self):
        deadline, last_progress, prior_seq, previous = (
            time.monotonic() + self.args.timeout,
            time.monotonic(),
            0,
            None,
        )
        while True:
            require(not self.daemon.exited(), "Daemon exited during scenario")
            self.sampler.check()
            require(not self.sampler.failure, str(self.sampler.failure))
            row = self.database.execute(
                "SELECT count(*),sum(state='stopped'),sum(state='failed'),sum(task_outcome='cancelled') FROM agents"
            ).fetchone()
            seq = self.database.execute("SELECT coalesce(max(seq),0) FROM events").fetchone()[0]
            if seq != prior_seq:
                last_progress, prior_seq = time.monotonic(), seq
            root_state = json.loads(
                self.database.execute(
                    "SELECT data FROM agent_records WHERE id=?", (self.root,)
                ).fetchone()[0]
            )
            progress = (*row, root_state["task_outcome"])
            if progress != previous:
                status = self.call("daemon", "status")
                append_jsonl_record(
                    self.output / "runtime-diagnostics.jsonl",
                    dict(
                        elapsed_seconds=time.monotonic() - self.began,
                        storage=status.get("storage"),
                        pool=status.get("runtime_resources"),
                        resident_actors=status.get("resident_actors"),
                        database_bytes=(self.state / "runtime/runtime.sqlite").stat().st_size,
                        wal_bytes=(
                            (self.state / "runtime/runtime.sqlite-wal").stat().st_size
                            if (self.state / "runtime/runtime.sqlite-wal").exists()
                            else 0
                        ),
                    ),
                    durable=False,
                )
                print(
                    json.dumps(
                        dict(
                            agents=row[0],
                            stopped=row[1],
                            failed=row[2],
                            cancelled=row[3],
                            pong_outcome=progress[-1],
                            elapsed_seconds=round(time.monotonic() - self.began, 2),
                        )
                    ),
                    flush=True,
                )
                previous = progress
            require(not row[2], "An agent failed; inspect saved database and daemon log")
            if row[0] == row[1] == self.args.agents and root_state["task_outcome"] == "completed":
                break
            require(time.monotonic() < deadline, "Scenario deadline exceeded")
            require(time.monotonic() - last_progress < 120, "No durable progress for 120 seconds")
            pause(1)
        self.elapsed = time.monotonic() - self.began
        self.observed_complete_ns = time.monotonic_ns()

    def shutdown(self):
        self.sampler.stop()
        self.call("daemon", "stop")
        require(self.daemon.wait(timeout=60) == 0, "Daemon cleanup failed")
        self.sampler.check()
        require(not self.sampler.failure, str(self.sampler.failure))
        require(not remaining_owned(self.sampler), "Owned mock processes survived daemon shutdown")
        with closing(sqlite3.connect(self.output / "runtime.sqlite")) as saved:
            self.database.backup(saved)

    def verify(self):
        self.traces = sorted(self.state.glob("profiles/*/provider/scale-mock/trace-*.jsonl"))
        require(self.traces, "Mock traces are missing")
        self.trace_dir = self.output / "mock-traces"
        self.trace_dir.mkdir()
        for trace in self.traces:
            atomic_copy(trace, self.trace_dir / trace.name)
        self.result = verify_database(
            self.output / "runtime.sqlite",
            self.root,
            self.args.agents,
            require_recycling=self.recycling_required,
        )
        manifest = self.result.pop("_manifest")
        atomic_write_text(
            self.output / "identity-manifest.json",
            json.dumps(manifest, separators=(",", ":")) + "\n",
        )
        self.result.update(
            verify_traces(
                list(self.trace_dir.glob("*.jsonl")), self.args.agents, self.limits, manifest
            )
        )
        trace_times = self.result.pop("trace_times_ns")
        first_start, last_turn, last_exit = (
            trace_times[name] for name in ("first_start", "last_turn", "last_exit")
        )
        self.result["timings"] = dict(
            daemon_ready_ns=self.daemon_ready_ns,
            first_provider_start_seconds=round((first_start - self.daemon_ready_ns) / 1e9, 6),
            last_turn_seconds=round((last_turn - self.daemon_ready_ns) / 1e9, 6),
            last_provider_exit_seconds=round((last_exit - self.daemon_ready_ns) / 1e9, 6),
            observed_completion_seconds=round(
                (self.observed_complete_ns - self.daemon_ready_ns) / 1e9, 6
            ),
            basis="Seconds after successful daemon status; trace endpoints use monotonic_ns; completion observation includes the one-second polling interval",
        )

    def prepare_report(self):
        samples = list(json_lines(self.output / "memory.jsonl"))
        peak = max(samples, key=lambda s: s["total_rss_kib"])
        self.result.update(
            result="passed",
            preset=self.args.preset,
            recycling_required=self.recycling_required,
            binary=str(self.args.binary.resolve()),
            elapsed_seconds=round(self.elapsed, 3),
            messages_per_second=round(self.result["messages"] / self.elapsed, 2),
            limits=self.limits,
            sampled_peak_rss_mib=round(peak["total_rss_kib"] / 1024, 2),
            sampled_peak_daemon_rss_mib=round(max(s["daemon_rss_kib"] for s in samples) / 1024, 2),
            memory_samples=len(samples),
            database_bytes=(self.output / "runtime.sqlite").stat().st_size,
            artifact_bytes=sum(
                p.stat().st_size for p in (self.state / "runtime/output").glob("*.json")
            ),
            memory_metric="Sampled sum of unique-process RSS; includes shared pages; mock providers only; may miss peaks",
        )
        shutil.rmtree(self.temporary)
        (self.output / "failure-state.txt").unlink(missing_ok=True)

    def report(self):
        atomic_write_text(
            self.output / "verification.json", json.dumps(self.result, indent=2) + "\n"
        )
        print(json.dumps(self.result), flush=True)
        self.success = True
        return self.result

    def cleanup(self):
        # This state is exclusively owned by this mock run. Stop it on every exit,
        # preserving evidence; never touch the user's real auth state or daemon.
        with finalizer(self.cleanup_state):
            try:
                cleanup_all(
                    self.stop_sampler, self.stop_daemon, self.save_database, self.close_owner
                )
            except BaseException:
                self.success = False
                raise

    def stop_sampler(self):
        if self.sampler:
            self.sampler.stop()

    def close_owner(self):
        resources = getattr(self, "resources", None)
        if resources is not None:
            resources.close()

    def kill_daemon(self):
        if self.daemon is not None:
            self.daemon.force()

    def stop_daemon(self):
        if self.daemon is not None and not self.daemon.exited():
            try:
                self.call("daemon", "stop")
                self.daemon.wait(timeout=60)
            except Exception as error:
                details = {
                    "graceful_error": str(error),
                    "owned_daemon_pid": self.daemon.pid,
                    "observed_owned_processes": list(self.sampler.owned) if self.sampler else [],
                }

                def record():
                    details["daemon_exit"] = self.daemon.returncode
                    details["remaining_owned"] = (
                        remaining_owned(self.sampler) if self.sampler else []
                    )
                    atomic_write_text(
                        self.output / "cleanup-error.json", json.dumps(details, indent=2) + "\n"
                    )

                with finalizer(record), finalizer(self.kill_daemon):
                    raise

    def save_database(self):
        if self.database:
            with finalizer(self.database.close):
                if not (self.output / "runtime.sqlite").exists():
                    with closing(sqlite3.connect(self.output / "runtime.sqlite")) as saved:
                        self.database.backup(saved)

    def cleanup_state(self):
        if self.success:
            shutil.rmtree(self.temporary)
        else:
            atomic_write_text(self.output / "failure-state.txt", str(self.temporary) + "\n")


def parse_args(argv=None):
    project = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agents", type=int, choices=(100, 1000, 10000, 100000), default=100)
    parser.add_argument("--binary", type=Path, default=product_default("target/release/hivectl"))
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument(
        "--preset",
        choices=PRESETS,
        default="high-residency",
        help="Mock-only limits: high-residency (default, 1024 slots/16 processes) or eviction (64/4); use --max-providers 1 to require resumption",
    )
    parser.add_argument("--warm-providers", type=int, default=0)
    parser.add_argument(
        "--max-loaded", type=int, help="Override the preset resident conversation limit"
    )
    parser.add_argument(
        "--max-providers", type=int, help="Override the preset physical process limit"
    )
    parser.add_argument(
        "--max-active-turns", type=int, help="Override the preset concurrent turn limit"
    )
    parser.add_argument(
        "--max-starting", type=int, help="Override the preset concurrent startup limit"
    )
    parser.add_argument(
        "--timeout",
        type=int,
        help="Scenario deadline in seconds (default: 7200 for 100000 agents, otherwise 1800)",
    )
    args = parser.parse_args(argv)
    require_paths(args, parser, "binary")
    if args.timeout is None:
        args.timeout = 7200 if args.agents == 100000 else 1800
    if args.timeout < 1:
        parser.error("--timeout must be positive")
    try:
        runtime_limits(args)
    except RuntimeError as error:
        parser.error(str(error))
    args.output_dir = (
        args.output_dir
        or product_root()
        / "dist"
        / "e2e"
        / f"mock-ping-pong-all-to-one-{args.preset}-{args.agents}-{uuid.uuid4()}"
    )
    return args


def main():
    args = parse_args()
    try:
        scenario = AllToOneRun(args).run()
        finish(scenario.report)
    except Exception as error:
        if getattr(args, "output_owned", False):
            failure = json.dumps({"result": "failed", "error": str(error)}, indent=2) + "\n"

            def record_failure():
                atomic_write_text(args.output_dir / "failure.json", failure)
                atomic_write_text(args.output_dir / "verification.json", failure)

            with finalizer(record_failure):
                raise
        raise


if __name__ == "__main__":
    from release.progress import entry

    raise SystemExit(entry(main, name="scenario"))
