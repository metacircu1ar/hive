"""Independent, bounded-memory verification of ping pong pairs and provider traces."""

import collections
import json
import math
import shutil
import sqlite3
from release.cancellation import check
from e2e.scripts.harness.audit import cancellable
import time
from contextlib import closing
from pathlib import Path

from e2e.scripts.harness.checks import one, require
from e2e.scripts.harness.evidence import index_events
from e2e.scripts.harness.io import json_lines
from e2e.scripts.harness.stats import histogram_summary
from e2e.scripts.harness.traces import SqliteTraceEvidence, TraceVerifier
from e2e.scripts.product_constants import TERMINAL_DRAIN_DEADLINE_MS, TERMINAL_DRAIN_QUIET_MS

SEED = "Start one ping pong pair."
CHILD = "Wait for ping from your parent."


def disk_budget(path):
    require(
        shutil.disk_usage(path).free > 4 * 1024**3,
        "Verifier has less than 4 GiB free disk; evidence retained",
    )


def verify_cleanup_observation(observation):
    metrics = ("quiet_ms", "elapsed_ms", "deadline_ms", "recleans", "late_items_tolerated")
    require(
        all(type(observation.get(k)) is int for k in metrics),
        "Invalid bounded quiet observation metrics",
    )
    require(
        observation.get("kind") == "observed_quiet"
        and TERMINAL_DRAIN_QUIET_MS
        <= observation["quiet_ms"]
        <= observation["elapsed_ms"]
        <= observation["deadline_ms"]
        == TERMINAL_DRAIN_DEADLINE_MS
        and observation["recleans"] >= 0
        and observation["late_items_tolerated"] >= 0,
        "Invalid bounded quiet observation",
    )


def verify_terminal_cleanup(grouped):
    seen = set()
    for event in grouped["terminal_cleanup_observed"]:
        data = event["data"]
        key = (event["execution"], data.get("turn_id"))
        require(key not in seen, "Duplicate terminal cleanup")
        seen.add(key)
        require(
            data.get("scope") == "conversation"
            and data.get("remaining") == 0
            and "next_cursor" in data
            and data["next_cursor"] is None,
            "Missing empty terminal inventory observation",
        )
        verify_cleanup_observation(data.get("observation", {}))
        finished = one(
            [e for e in grouped["turn_finished"] if (e["execution"], e["data"]["turn_id"]) == key],
            "cleanup turn",
        )
        require(finished["seq"] < event["seq"], "Cleanup predates turn completion")
        require(
            finished["data"]["status"] == "interrupted"
            or any(e["seq"] < finished["seq"] for e in grouped["termination_requested"]),
            "Cleanup without interruption intent",
        )
        require(
            not any(finished["seq"] < e["seq"] < event["seq"] for e in grouped["turn_started"]),
            "Turn admitted before terminal cleanup",
        )


def verify_database(runtime, submissions, count, audit):
    """Read two agents at a time; global uniqueness/trace manifest lives on disk."""
    with closing(sqlite3.connect(f"file:{runtime}?mode=ro", uri=True)) as db, cancellable(db):
        return PairDatabaseAudit(db, audit, runtime, count).verify(submissions)


class PairDatabaseAudit:
    """Global counts and disk-backed uniqueness; only one pair is resident."""

    expect_initialization = True

    def __init__(self, db, audit, runtime, count):
        self.db, self.audit, self.runtime, self.count = db, audit, runtime, count

    def scalar(self, sql, args=()):
        return self.db.execute(sql, args).fetchone()[0]

    def verify(self, submissions):
        self.db.row_factory = sqlite3.Row
        self.db.execute("PRAGMA cache_size=-16384")
        self.db.execute("PRAGMA temp_store=FILE")
        for entry in json_lines(submissions):
            check()
            self.audit.execute(
                "INSERT INTO seeds VALUES(?,?)",
                (entry["agent_id"], entry["message_id"]),
            )
        self.verify_counts()
        self.verify_events()
        self.verify_providers()
        self.latencies = {
            name: collections.Counter()
            for name in (
                "ping_to_pong_seconds",
                "pair_lifetime_seconds",
                "seed_queue_seconds",
                "reply_accepted_to_turn_started_seconds",
                "child_seed_queue_seconds",
            )
        }
        self.first_pong = self.last_pong = None
        self.first_spawn = self.scalar("SELECT min(at) FROM events WHERE kind='spawned'")
        self.completed_pairs = self.resumed_agents = self.pre_session_releases = 0
        for parent_row in self.db.execute(
            "SELECT r.data FROM agents a INDEXED BY agents_listing JOIN agent_records r ON r.id=a.id WHERE a.parent_id IS NULL ORDER BY a.listing_seq"
        ):
            check()
            PairEvidence(self, json.loads(parent_row[0])).verify()
        return self.result()

    def verify_counts(self):
        require(
            self.audit.execute("SELECT count(*) FROM seeds").fetchone()[0] == self.count // 2,
            "Wrong seed count",
        )
        require(self.scalar("SELECT count(*) FROM agents") == self.count, "Wrong agent count")
        require(
            self.scalar("SELECT count(*) FROM messages") == 2 * self.count, "Wrong message count"
        )
        require(
            self.scalar("SELECT count(*) FROM messages WHERE mode!='next-turn'") == 0,
            "Unexpected steering",
        )
        require(self.scalar("SELECT active FROM runtime_counts") == 0, "Active capacity leaked")
        # Recompute from messages/agent state rather than trusting derived counters.
        require(
            not self.db.execute("""
            SELECT 1 FROM agents a WHERE
              a.pending_count != (SELECT count(*) FROM messages WHERE recipient=a.id AND state='accepted' AND mode='next-turn')
              OR a.mail_count != (SELECT count(*) FROM messages WHERE recipient=a.id AND state='accepted' AND mode='next-turn' AND startup=0)
              OR a.pending_seq IS NOT (SELECT min(seq) FROM messages WHERE recipient=a.id AND state='accepted' AND mode='next-turn')
              OR a.head_startup IS NOT (SELECT startup FROM messages WHERE recipient=a.id AND state='accepted' AND mode='next-turn' ORDER BY seq LIMIT 1)
              OR a.admission_class != CASE WHEN (SELECT startup FROM messages WHERE recipient=a.id AND state='accepted' AND mode='next-turn' ORDER BY seq LIMIT 1)=0
                  THEN CASE WHEN a.coordinator=1 THEN 0 ELSE 1 END
                  WHEN a.parent_id IS NOT NULL THEN 2 ELSE 3 END
            LIMIT 1
        """).fetchone(),
            "Mailbox counters disagree with messages",
        )
        require(
            self.scalar("SELECT active FROM runtime_counts")
            == self.scalar("""
            SELECT count(*) FROM agents WHERE NOT(task_outcome IN ('completed','cancelled') OR expired=1) OR state!='stopped'
        """),
            "Active counter disagrees with agents",
        )
        require(
            not self.db.execute("""
            SELECT 1 FROM agents a WHERE a.runnable IS NOT (
              claimed=0 AND blocked=0 AND paused=0 AND expired=0 AND task_outcome NOT IN ('completed','cancelled')
              AND state IN ('starting','idle','stopped') AND control IS NULL AND completion_pending IS NULL
              AND (state='starting' OR EXISTS(SELECT 1 FROM messages WHERE recipient=a.id AND state='accepted' AND mode='next-turn'))
            ) LIMIT 1
        """).fetchone(),
            "Readiness disagrees with messages",
        )

    def verify_events(self):
        allowed = {
            "completion_requested",
            "execution_starting",
            "execution_stopped",
            "message_accepted",
            "provider_session_released",
            "provider_shutdown",
            "provider_started",
            "session_ready",
            "spawned",
            "task_completed",
            "termination_requested",
            "terminal_cleanup_observed",
            "thread_bound",
            "tool_completed",
            "turn_finished",
            "turn_started",
        }
        self.counts = dict(self.db.execute("SELECT kind,count(*) FROM events GROUP BY kind"))
        require(set(self.counts) <= allowed, "Unexpected runtime event kind")
        require(
            not any(
                self.counts.get(k)
                for k in ("execution_failed", "model_rerouted", "session_quarantined")
            ),
            "Execution failure was hidden",
        )
        require(
            not self.db.execute(
                "SELECT 1 FROM events e WHERE NOT EXISTS (SELECT 1 FROM agents a WHERE a.id=e.agent) LIMIT 1"
            ).fetchone(),
            "Orphan event",
        )
        for kind, expected in [
            ("spawned", self.count),
            ("turn_started", 2 * self.count),
            ("turn_finished", 2 * self.count),
            ("message_accepted", self.count),
            ("completion_requested", self.count // 2),
            ("task_completed", self.count // 2),
            ("termination_requested", self.count // 2),
        ]:
            require(self.counts.get(kind, 0) == expected, f"Wrong {kind} count")

    def verify_providers(self):
        for row in self.db.execute("SELECT * FROM provider_events ORDER BY seq"):
            check()
            allowed = (
                ("started", "initialized", "shutdown")
                if self.expect_initialization
                else ("started", "shutdown")
            )
            require(row["kind"] in allowed, "Unexpected provider event kind")
            data = json.loads(row["data"])
            if row["kind"] == "started":
                self.audit.execute(
                    "INSERT INTO providers(id,pid,seq) VALUES(?,?,?)",
                    (row["provider"], data["pid"], row["seq"]),
                )
            elif row["kind"] == "initialized":
                require(
                    data
                    == {
                        "compatibility": {
                            "registry": {
                                "corpus_versions": ["0.154.0", "0.161.0"],
                                "last_tested": "0.161.0",
                                "minimum": "0.154.0",
                                "provider": "codex",
                            },
                            "support": "untested",
                            "version": "0.163.0",
                        },
                        "reply": {},
                    },
                    "Unexpected mock provider compatibility observation",
                )
                require(
                    self.audit.execute(
                        "UPDATE providers SET initialized=1 WHERE id=? AND initialized=0 AND ended=0",
                        (row["provider"],),
                    ).rowcount
                    == 1,
                    "Duplicate/unknown/out-of-order provider initialization",
                )
            elif row["kind"] == "shutdown":
                shutdown = data["shutdown"]
                require(
                    data.get("error_code") is None
                    and shutdown["confirmed"]
                    and shutdown["quiescent"]
                    and shutdown["exit_code"] == 0,
                    "Unconfirmed provider cleanup",
                )
                require(
                    self.audit.execute(
                        "UPDATE providers SET ended=1 WHERE id=? AND ended=0",
                        (row["provider"],),
                    ).rowcount
                    == 1,
                    "Duplicate/unknown provider shutdown",
                )
        self.providers = self.audit.execute("SELECT count(*) FROM providers").fetchone()[0]
        require(
            self.providers
            and not self.audit.execute("SELECT 1 FROM providers WHERE ended=0").fetchone(),
            "Leaked provider",
        )
        if self.expect_initialization:
            require(
                not self.audit.execute("SELECT 1 FROM providers WHERE initialized=0").fetchone(),
                "Missing provider initialization",
            )

    def result(self):
        require(self.completed_pairs == self.count // 2, "Wrong root count")
        for kind, table in [
            ("provider_started", "executions"),
            ("session_ready", "sessions"),
        ]:
            require(
                self.counts.get(kind, 0)
                == self.audit.execute(f"SELECT count(*) FROM {table}").fetchone()[0],
                "Unattributed session/execution event",
            )
        self.audit.commit()
        return dict(
            agents=self.count,
            pairs=self.completed_pairs,
            messages=2 * self.count,
            turns=2 * self.count,
            pings=self.completed_pairs,
            pongs=self.completed_pairs,
            explicit_cancellations=self.completed_pairs,
            self_completions=self.completed_pairs,
            native_conversations=self.count,
            resumed_agents=self.resumed_agents,
            pre_session_releases=self.pre_session_releases,
            confirmed_provider_shutdowns=self.providers,
            first_pong_seconds=self.first_pong - self.first_spawn,
            last_pong_seconds=self.last_pong - self.first_spawn,
            latencies={name: histogram_summary(hist) for name, hist in self.latencies.items()},
        )


class PairEvidence:
    """Validate one parent/child exchange before releasing its evidence."""

    def __init__(self, totals, parent):
        self.totals, self.parent = totals, parent
        children = list(
            self.totals.db.execute(
                "SELECT data FROM agent_records WHERE id IN (SELECT id FROM agents WHERE parent_id=?)",
                (self.parent["id"],),
            )
        )
        self.child = json.loads(one(children, "pair child")[0])
        self.pair = [self.parent, self.child]
        (
            self.events,
            self.messages,
            self.starts,
            self.finishes,
            self.accepts,
            self.spawn,
            self.executions,
        ) = (
            {},
            {},
            {},
            {},
            {},
            {},
            {},
        )

    def verify(self):
        for agent in self.pair:
            self.agent(agent)
        self.identities()
        self.turns()
        self.message_order()
        self.completion()
        self.latency_totals()

    def agent(self, a):
        aid = a["id"]
        grouped, ready = self.agent_events(a, aid)
        self.started_executions(aid, grouped)
        self.ready_sessions(a, aid, ready)
        self.pre_session_releases(aid, grouped)
        self.deliveries(aid, grouped, ready)

    def agent_events(self, a, aid):
        require(
            a["state"] == "stopped"
            and a["turn_id"] is None
            and a["control"] is None
            and a["completion_pending"] is None
            and a["error"] is None
            and not a["expired"]
            and not a["paused"],
            "Agent did not stop cleanly",
        )
        require(
            a["model"] == "gpt-5.6-luna" and a["effort"] == "low",
            "Wrong model/effort",
        )
        self.totals.audit.execute(
            "INSERT INTO threads(thread,agent) VALUES(?,?)",
            (a["thread_id"], aid),
        )
        grouped = index_events(
            self.totals.db.execute(
                "SELECT seq,execution,at,kind,data FROM events WHERE agent=? ORDER BY seq", (aid,)
            ),
            include_at=True,
        )
        self.events[aid] = grouped
        verify_terminal_cleanup(grouped)
        self.spawn[aid] = one(grouped["spawned"], "agent spawn")
        require(
            self.spawn[aid]["data"]["parent_id"] == a["parent_id"]
            and self.spawn[aid]["data"]["role"] == a["role"],
            "Wrong spawn identity",
        )
        ready = grouped["session_ready"]
        require(ready, "Missing native conversation")
        return grouped, ready

    def started_executions(self, aid, grouped):
        for event in grouped["provider_started"]:
            require(event["execution"] is not None, "Missing execution identity")
            require(
                event["execution"] not in self.executions,
                "Duplicate execution identity",
            )
            provider = event["data"]["provider_id"]
            require(
                self.totals.audit.execute(
                    "SELECT 1 FROM providers WHERE id=?", (provider,)
                ).fetchone(),
                "Unknown provider generation",
            )
            self.totals.audit.execute("INSERT INTO executions VALUES(?)", (event["execution"],))
            self.executions[event["execution"]] = dict(
                agent=aid, provider=provider, started=event["seq"], ready=None
            )

    def ready_sessions(self, a, aid, ready):
        for event in ready:
            d = event["data"]
            require(
                d["thread_id"] == a["thread_id"]
                and d["model"] == a["model"]
                and d["effort"] == a["effort"],
                "Conversation/model changed",
            )
            execution = self.executions[event["execution"]]
            require(
                execution["agent"] == aid
                and execution["ready"] is None
                and execution["started"] < event["seq"],
                "Wrong session execution binding",
            )
            execution["ready"] = event["seq"]
            self.totals.audit.execute(
                "INSERT INTO sessions(provider,thread,resumed) VALUES(?,?,?)",
                (execution["provider"], a["thread_id"], int(d["resumed"])),
            )

    def pre_session_releases(self, aid, grouped):
        for execution_id, execution in self.executions.items():
            if execution["agent"] != aid or execution["ready"] is not None:
                continue
            # A cohort may retire after assignment but before any native
            # conversation exists. This is not a delivered/resumed session.
            require(
                not any(
                    e["execution"] == execution_id
                    for kind in (
                        "thread_bound",
                        "turn_started",
                        "turn_finished",
                        "tool_completed",
                    )
                    for e in grouped[kind]
                ),
                "Pre-session release contains native work",
            )
            stopped = one(
                [e for e in grouped["execution_stopped"] if e["execution"] == execution_id],
                "pre-session stop",
            )
            released = [
                e
                for kind in ("provider_session_released", "provider_shutdown")
                for e in grouped[kind]
                if e["execution"] == execution_id
                and execution["started"] < e["seq"] < stopped["seq"]
            ]
            require(
                released
                and all(e["data"]["confirmed"] and e["data"]["quiescent"] for e in released),
                "Unproven pre-session release",
            )
            self.totals.pre_session_releases += 1

    def deliveries(self, aid, grouped, ready):
        require(
            sum(not e["data"]["resumed"] for e in ready) == 1 and not ready[0]["data"]["resumed"],
            "Invalid conversation resume history",
        )
        self.totals.resumed_agents += len(ready) > 1
        require(
            len(grouped["turn_started"]) == len(grouped["turn_finished"]) == 2,
            "Wrong per-agent delivery count",
        )
        for kind, target in [
            ("turn_started", self.starts),
            ("turn_finished", self.finishes),
        ]:
            for event in grouped[kind]:
                mid = event["data"]["message_id"]
                require(mid not in target, "Duplicate turn lifecycle")
                target[mid] = (aid, event)
        acceptance = one(grouped["message_accepted"], "pair message acceptance")
        self.accepts[acceptance["data"]["id"]] = (aid, acceptance)
        for row in self.totals.db.execute(
            "SELECT * FROM messages WHERE recipient=? ORDER BY seq", (aid,)
        ):
            require(row["id"] not in self.messages, "Duplicate message identity")
            self.messages[row["id"]] = dict(row)

    def identities(self):
        require(
            self.parent["role"] == "pair-ping"
            and self.parent["parent_id"] is None
            and self.parent["lifetime"] == "independent"
            and self.parent["task_outcome"] == "completed",
            "Invalid pair parent",
        )
        require(
            self.child["role"] == "pair-pong"
            and self.child["parent_id"] == self.parent["id"]
            and self.child["lifetime"] == "independent"
            and self.child["task_outcome"] == "cancelled",
            "Invalid pair child",
        )
        require(
            len(self.messages) == 4
            and set(self.messages) == set(self.starts) == set(self.finishes),
            "Missing/extra pair messages",
        )
        by_body = {m["body"]: m for m in self.messages.values()}
        require(set(by_body) == {SEED, CHILD, "ping", "pong"}, "Unexpected pair message")
        self.seed, self.startup, self.ping, self.pong = (
            by_body[k] for k in (SEED, CHILD, "ping", "pong")
        )
        require(
            self.totals.audit.execute(
                "SELECT message FROM seeds WHERE agent=?", (self.parent["id"],)
            ).fetchone()
            == (self.seed["id"],),
            "Unsubmitted root",
        )

    def turns(self):
        for m, sender, recipient, initial in [
            (self.seed, "human", self.parent["id"], 1),
            (self.startup, self.parent["id"], self.child["id"], 1),
            (self.ping, self.parent["id"], self.child["id"], 0),
            (self.pong, self.child["id"], self.parent["id"], 0),
        ]:
            require(
                m["sender"] == sender and m["recipient"] == recipient and m["startup"] == initial,
                "Cross-pair message or bad startup",
            )
            aid, begin = self.starts[m["id"]]
            end_aid, end = self.finishes[m["id"]]
            require(
                aid == end_aid == recipient
                and begin["seq"] < end["seq"]
                and begin["execution"] == end["execution"]
                and begin["data"]["turn_id"] == end["data"]["turn_id"]
                and end["data"]["status"] == m["state"]
                and begin["data"]["model"] == "gpt-5.6-luna"
                and begin["data"]["effort"] == "low",
                "Invalid turn lifecycle",
            )
            thread = (
                self.parent["thread_id"]
                if recipient == self.parent["id"]
                else self.child["thread_id"]
            )
            execution = self.executions[begin["execution"]]
            require(
                execution["agent"] == aid
                and execution["ready"] is not None
                and execution["ready"] < begin["seq"],
                "Turn uses another agent execution",
            )
            self.totals.audit.execute(
                "INSERT INTO turns(turn,message,thread,provider,status,pair,kind) VALUES(?,?,?,?,?,?,?)",
                (
                    begin["data"]["turn_id"],
                    m["id"],
                    thread,
                    execution["provider"],
                    m["state"],
                    self.parent["id"],
                    m["body"],
                ),
            )

    def message_order(self):
        require(
            self.spawn[self.parent["id"]]["data"]["initial_message_id"] == self.seed["id"]
            and self.spawn[self.child["id"]]["data"]["initial_message_id"] == self.startup["id"],
            "Wrong startup binding",
        )
        require(
            self.seed["state"] == self.startup["state"] == self.pong["state"] == "completed",
            "Undelivered required message",
        )
        for a, initial in ((self.parent, self.seed), (self.child, self.startup)):
            require(
                self.spawn[a["id"]]["seq"] < self.starts[initial["id"]][1]["seq"]
                and self.spawn[a["id"]]["data"]["body"] == initial["body"],
                "Startup precedes spawn or body differs",
            )
        require(
            self.finishes[self.seed["id"]][1]["seq"] < self.starts[self.pong["id"]][1]["seq"]
            and self.finishes[self.startup["id"]][1]["seq"]
            < self.starts[self.ping["id"]][1]["seq"],
            "Agent turns overlap",
        )
        require(
            set(self.accepts) == {self.ping["id"], self.pong["id"]}, "Wrong acceptance identities"
        )
        for sent, trigger in ((self.ping, self.seed), (self.pong, self.ping)):
            aid, accepted = self.accepts[sent["id"]]
            sender, turn = self.starts[trigger["id"]]
            require(
                aid == sent["recipient"]
                and sender == sent["sender"]
                and accepted["data"]["sender"] == sender
                and accepted["data"]["sender_turn_id"] == turn["data"]["turn_id"]
                and turn["seq"] < accepted["seq"] < self.finishes[trigger["id"]][1]["seq"]
                and accepted["seq"] < self.starts[sent["id"]][1]["seq"],
                "Wrong sending-turn provenance",
            )
        require(
            self.starts[self.seed["id"]][1]["seq"]
            < self.spawn[self.child["id"]]["seq"]
            < self.accepts[self.ping["id"]][1]["seq"]
            < self.finishes[self.seed["id"]][1]["seq"],
            "Spawn/send ordering is wrong",
        )

    def completion(self):
        require(
            not self.events[self.parent["id"]]["termination_requested"]
            and not self.events[self.child["id"]]["completion_requested"]
            and not self.events[self.child["id"]]["task_completed"],
            "Wrong completion actor",
        )
        cancellation = one(
            self.events[self.child["id"]]["termination_requested"], "child cancellation"
        )
        completion = one(
            self.events[self.parent["id"]]["completion_requested"],
            "parent completion request",
        )
        self.completed = one(self.events[self.parent["id"]]["task_completed"], "parent completion")
        require(
            cancellation["data"]["by"] == completion["data"]["by"] == self.parent["id"],
            "Cross-pair termination/completion",
        )
        require(
            self.starts[self.pong["id"]][1]["seq"]
            < cancellation["seq"]
            < completion["seq"]
            < self.completed["seq"]
            and completion["seq"]
            < self.finishes[self.pong["id"]][1]["seq"]
            < self.completed["seq"],
            "Invalid pair completion order",
        )
        require(self.ping["state"] in ("completed", "interrupted"), "Undelivered ping")
        if self.ping["state"] == "interrupted":
            require(
                self.accepts[self.pong["id"]][1]["seq"]
                < cancellation["seq"]
                < self.finishes[self.ping["id"]][1]["seq"],
                "Interrupted ping without reply and cancellation",
            )

    def latency_totals(self):
        pong_at = self.accepts[self.pong["id"]][1]["at"]
        self.totals.first_pong = (
            pong_at if self.totals.first_pong is None else min(self.totals.first_pong, pong_at)
        )
        self.totals.last_pong = (
            pong_at if self.totals.last_pong is None else max(self.totals.last_pong, pong_at)
        )
        for name, delta in [
            ("ping_to_pong_seconds", pong_at - self.accepts[self.ping["id"]][1]["at"]),
            (
                "reply_accepted_to_turn_started_seconds",
                self.starts[self.pong["id"]][1]["at"] - pong_at,
            ),
            (
                "child_seed_queue_seconds",
                self.starts[self.startup["id"]][1]["at"] - self.spawn[self.child["id"]]["at"],
            ),
            ("pair_lifetime_seconds", self.completed["at"] - self.spawn[self.parent["id"]]["at"]),
            (
                "seed_queue_seconds",
                self.starts[self.seed["id"]][1]["at"] - self.spawn[self.parent["id"]]["at"],
            ),
        ]:
            self.totals.latencies[name][delta] += 1
        self.totals.completed_pairs += 1
        if self.totals.completed_pairs % 1000 == 0:
            disk_budget(Path(self.totals.runtime).parent)
            self.totals.audit.commit()
        if self.totals.completed_pairs % 10000 == 0:
            print(
                json.dumps(
                    dict(
                        phase="database_verification",
                        verified_pairs=self.totals.completed_pairs,
                    )
                ),
                flush=True,
            )


SCHEMA = """
CREATE TABLE seeds(agent TEXT PRIMARY KEY, message TEXT UNIQUE NOT NULL);
CREATE TABLE providers(id TEXT PRIMARY KEY,pid INTEGER NOT NULL,seq INTEGER NOT NULL,ended INTEGER NOT NULL DEFAULT 0,initialized INTEGER NOT NULL DEFAULT 0);
CREATE INDEX provider_pid ON providers(pid,seq);
CREATE TABLE executions(execution TEXT PRIMARY KEY NOT NULL);
CREATE TABLE threads(thread TEXT PRIMARY KEY NOT NULL,agent TEXT UNIQUE NOT NULL,seen INTEGER NOT NULL DEFAULT 0);
CREATE TABLE sessions(provider TEXT,thread TEXT,resumed INTEGER,seen INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(provider,thread));
CREATE TABLE turns(turn TEXT PRIMARY KEY,message TEXT UNIQUE,thread TEXT,provider TEXT,status TEXT,pair TEXT,kind TEXT,started INTEGER DEFAULT 0,ended INTEGER DEFAULT 0,start_ns INTEGER,end_ns INTEGER);
CREATE INDEX turns_pair ON turns(pair,kind);
CREATE TABLE processes(process TEXT PRIMARY KEY,pid INTEGER,ns INTEGER,provider TEXT);
CREATE TABLE frames(ns INTEGER,data TEXT);
"""


def trace_latencies(audit):
    precise = {}
    for name, sql, args in [
        (
            "seed_turn_start_to_final_pong_turn_end_seconds",
            "SELECT (reply.end_ns-seed.start_ns)/1e9 FROM turns seed JOIN turns reply ON reply.pair=seed.pair AND reply.kind='pong' WHERE seed.kind=? ORDER BY reply.end_ns-seed.start_ns",
            (SEED,),
        ),
        (
            "child_ping_turn_start_to_parent_pong_turn_start_seconds",
            "SELECT (reply.start_ns-ping.start_ns)/1e9 FROM turns ping JOIN turns reply ON reply.pair=ping.pair AND reply.kind='pong' WHERE ping.kind='ping' ORDER BY reply.start_ns-ping.start_ns",
            (),
        ),
    ]:
        count = audit.execute("SELECT count(*) FROM seeds").fetchone()[0]
        ranks = {
            key: max(1, math.ceil(count * fraction))
            for key, fraction in [
                ("p50", 0.5),
                ("p95", 0.95),
                ("p99", 0.99),
                ("max", 1),
            ]
        }
        values = dict(count=count)
        total = 0.0
        for index, (value,) in enumerate(audit.execute(sql, args), 1):
            check()
            require(value >= 0, "Monotonic trace ordering failed")
            total += value
            for key, rank in ranks.items():
                if index == rank:
                    values[key] = value
        require(index == count, "Incomplete precise latency data")
        values["mean"] = total / count
        precise[name] = values
    return precise


def verify_traces(paths, limits, audit):
    with cancellable(audit):
        return _verify_traces(paths, limits, audit)


def _verify_traces(paths, limits, audit):
    # SQLite orders frames on disk with one trace file open at a time.
    result = TraceVerifier(SqliteTraceEvidence(audit), limits).verify(paths, check_disk=disk_budget)
    peaks = {name: value for name, value in result.items() if name.startswith("peak_")}
    return {
        "trace_latencies": trace_latencies(audit),
        **peaks,
        "peak_mock_processes_scope": "runtime session processes; excludes catalog discovery and CLI login",
        "overlap_barriers": result["overlap_barriers"],
        "trace_times_ns": result["trace_times_ns"],
    }


def verify(runtime, submissions, traces, count, limits, audit_path):
    began = time.monotonic()
    require(not audit_path.exists(), "Verifier scratch already exists")
    with closing(sqlite3.connect(audit_path)) as audit, cancellable(audit):
        audit.execute("PRAGMA journal_mode=OFF")
        audit.execute("PRAGMA synchronous=OFF")
        audit.execute("PRAGMA temp_store=FILE")
        audit.execute("PRAGMA cache_size=-32768")
        audit.executescript(SCHEMA)
        result = verify_database(runtime, submissions, count, audit)
        after_db = time.monotonic()
        result.update(verify_traces(traces, limits, audit))
        result["verification_seconds"] = dict(
            database_verification_seconds=after_db - began,
            trace_verification_seconds=time.monotonic() - after_db,
        )
    return result
