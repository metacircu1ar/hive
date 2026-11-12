#!/usr/bin/env python3
"""Real Codex/Claude ping-pong pairs, with a fully resident memory barrier."""

import argparse
import hashlib
import json
import platform
import sqlite3
import time
import uuid
from pathlib import Path

from e2e.scripts.harness.checks import require
from e2e.scripts.harness.cli import call_deadline, event_pages
from release.files import atomic_write_json, atomic_write_text, create_directory
from release.progress import cleanup_all, entry, finalizer, phase
from e2e.scripts.live_pair_memory import FootprintSampler, distribution, summarize
from e2e.scripts.product_constants import LIVE_CLAUDE_MODEL, LIVE_CODEX_EFFORT, LIVE_CODEX_MODEL
from e2e.paths import output_directory, product_default, require_paths
from e2e.live.ownership import (
    Daemon,
    campaign,
    cancel_descendants,
    configured_cli,
    default_state,
    opt_in,
    owned_agents,
    verify_tasks,
)
from release.cancellation import deadline, finish, pause
from e2e.scripts.verify_live_ping_pong_pair import verify


PING = """You are one ping parent in a Hive protocol measurement. Use ONLY Hive
coordination tools, never native subagents, approvals, shell, files or network.
Do not poll or wait inside a turn. End each turn promptly after its required calls.
Apply only the rule for the current input, not rules for future turns.
Initial input is INIT <nonce> [PREFLIGHT]. Call hive_spawn exactly once with role
live-pong, lifetime independent, body EXACTLY the initial input, and request_id
spawn-<nonce>. Address your sole child as @child; never copy its ID. Then output READY and end this
turn WITHOUT hive_complete. Do not send a ping yet.
On START <nonce>, call hive_send once with target @child and body PING <nonce> and
request_id ping-<nonce>. Then output SENT and end the turn WITHOUT completing.
On PONG <nonce> from that child, only after your START turn sent PING, call hive_terminate with target @child and request_id
stop-<nonce>, then hive_complete with body DONE <nonce>, request_id done-<nonce>.
No other agents or messages. If a call fails, report it and end the turn; do not
substitute tools or silently retry. Request IDs and message bodies are literal
substitutions without angle brackets, quotes or extra prose.
"""
PONG = """You are one pong child in a Hive protocol measurement. Use ONLY Hive
coordination tools, never native subagents, approvals, shell, files or network.
Apply only the rule for the current input, not rules for future turns.
On initial INIT <nonce> [PREFLIGHT], address your parent as @parent; never copy its ID.
In an INIT turn, call no tool except hive_agents with limit 1, and call that only if
the current input ends in PREFLIGHT. Output READY and end the INIT turn. Never send
a message or complete the task in an INIT turn.
On PING <nonce>, call hive_send exactly once with target @parent and body
PONG <nonce>, request_id pong-<nonce>. Output SENT and end this turn. The parent
will terminate you. Never call hive_complete, spawn, terminate or send other messages.
Do not poll or wait. If a tool fails, report it and end the turn, without retry.
"""
PHASES = dict(preflight="verify", startup="scenario", resident_hold="observe", exchange="wait")


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=product_default("target/release/hivectl"))
    parser.add_argument("--state-dir", type=Path, default=default_state())
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--live", action="store_true")
    parser.add_argument("--providers", choices=("codex", "claude", "mixed"), default="codex")
    parser.add_argument("--codex-model", default=LIVE_CODEX_MODEL)
    parser.add_argument("--claude-model", default=LIVE_CLAUDE_MODEL)
    parser.add_argument("--pairs", type=int, default=1)
    parser.add_argument("--max-starting", type=int, default=8)
    parser.add_argument("--timeout", type=int)
    args = parser.parse_args()
    if not 1 <= args.pairs <= 50:
        parser.error("Use 1..50 pairs")
    cap = 600 if args.pairs == 1 else 1800
    args.timeout = cap if args.timeout is None else args.timeout
    if not 1 <= args.timeout <= cap:
        parser.error(f"This pair count requires a timeout of 1..{cap} seconds")
    if not 1 <= args.max_starting <= 100:
        parser.error("--max-starting must be 1..100 (capped at agent count)")
    args.output_dir = args.output_dir or output_directory("live-pairs")
    selected = {"codex", "claude"} if args.providers == "mixed" else {args.providers}
    opt_in(
        parser,
        args,
        agents=2 * args.pairs,
        models={
            key: value
            for key, value in dict(
                codex=dict(model=args.codex_model, effort=LIVE_CODEX_EFFORT),
                claude=dict(model=args.claude_model, effort="low"),
            ).items()
            if key in selected
        },
    )
    require_paths(args, parser, "binary")
    return args


class Run:
    def __init__(self, args, admission=None):
        self.args = args
        self.admission = admission
        self.output, self.state = args.output_dir.resolve(), args.state_dir.resolve()
        self.binary = args.binary.resolve()
        require(self.binary.parent.name == "release", "Use a plain release build")
        require(not self.output.exists(), "Use a fresh output directory")
        create_directory(self.output, mode=0o700)
        self.work = self.output / "work"
        create_directory(self.work, mode=0o700)
        self.config = self.output / "config.json"
        runtime = dict(
            max_agents=2 * args.pairs,
            max_loaded=2 * args.pairs,
            max_providers=2 * args.pairs,
            max_active_turns=2 * args.pairs,
            max_starting=min(args.max_starting, 2 * args.pairs),
            idle_seconds=max(1800, args.timeout),
            warm_providers=0,
            linger_seconds=0,
        )
        roles = {}
        self.providers = dict(
            zip(
                ("live-ping", "live-pong"),
                ("codex", "claude")
                if args.providers == "mixed"
                else (args.providers, args.providers),
                strict=True,
            )
        )
        self.expected = {}
        for role, instructions in (("live-ping", PING), ("live-pong", PONG)):
            provider = self.providers[role]
            model = args.codex_model if provider == "codex" else args.claude_model
            effort = LIVE_CODEX_EFFORT if provider == "codex" else "low"
            path = self.output / (role + ".md")
            atomic_write_text(path, instructions)
            roles[role] = dict(
                auth=provider + "-main",
                model=model,
                effort=effort,
                instructions=str(path),
                lifetime="independent",
                sandbox="read-only",
                api_fallback=False,
            )
            self.expected[role] = dict(model=model, effort=effort)
        atomic_write_json(
            self.config,
            dict(
                runtime=runtime,
                roles=roles,
                auth_profiles={
                    p + "-main": dict(provider=p, method="subscription")
                    for p in ("codex", "claude")
                },
            ),
        )
        remote = [str(self.binary), "--state-dir", str(self.state)]
        self.call = configured_cli(remote, self.config, timeout=60)
        self.roots, self.initial = {}, {}
        self.pending_spawns = set()
        self.timings, self.observations = {}, []
        self.sampler = self.database = self.daemon = None
        self.daemon_owner = Daemon(self.call, self.state, self.config, self.output)
        self.success = False
        self.began = time.monotonic()
        self.deadline = self.began + args.timeout
        self.run_id = uuid.uuid4().hex
        self.save(
            "build",
            dict(
                profile="release",
                binary=str(self.binary),
                binary_sha256=hashlib.sha256(self.binary.read_bytes()).hexdigest(),
                host=platform.platform(),
                wall_start=time.time(),
            ),
        )

    def save(self, name, value):
        atomic_write_json(self.output / (name + ".json"), value)

    def discover(self):
        return owned_agents(self.call, self.work, self.roots)

    def check(self, agents):
        require(time.monotonic() < self.deadline, "Live run deadline reached; no automatic replay")
        require(not self.sampler.failure, self.sampler.failure)
        require(
            not any(a["state"] == "failed" or a["error"] is not None for a in agents),
            "Provider failure; retain evidence, do not retry",
        )
        require(not any(a.get("waiting_reason") for a in agents), "Provider waiting/quota blocked")

    def start(self):
        # Check all retained work before an auth probe or owned daemon can run.
        from e2e.live.ownership import preflight

        preflight(self.state)
        for provider in sorted(set(self.providers.values())):
            require(
                self.call("auth", "status", provider + "-main")["state"] == "authenticated",
                "An existing authenticated login is required; no login is attempted",
            )
        dbpath = self.state / "runtime/runtime.sqlite"
        self.daemon = self.daemon_owner.start(self.admission)
        self.sampler = FootprintSampler(self.daemon["pid"], self.output, max_rss_gib=32)
        self.sampler.start()
        status = self.call("daemon", "status")
        self.save("resolved-runtime", status["runtime_resources"])
        require(
            status["runtime_resources"]["limits"]["sessions_per_codex_process"] == 16,
            "Shipping default must resolve to 16 conversations per Codex process",
        )
        self.database = sqlite3.connect(dbpath.as_uri() + "?mode=ro", uri=True)
        self.timings["daemon_ready"] = time.monotonic()
        self.save("daemon", self.daemon)

    def batch(self, name, operations):
        path = self.output / (name + "-intent.json")
        atomic_write_json(path, operations, create_once=True)
        before = time.monotonic()
        if operations and all(operation["op"] == "spawn" for operation in operations):
            self.pending_spawns.add(name)
        reply = self.call(
            "batch", "--operations-file", str(path), "--request-id", self.run_id + "-" + name
        )
        self.save(name + "-reply", reply)
        self.timings[name] = [before, time.monotonic()]
        return reply["results"]

    def spawn(self, indices, name):
        operations, nonces = [], []
        for index in indices:
            nonce = self.run_id + f"-{index:03}"
            body = "INIT " + nonce + (" PREFLIGHT" if index == 0 else "")
            self.initial[nonce] = body
            nonces.append(nonce)
            operations.append(
                dict(
                    op="spawn",
                    role="live-ping",
                    cwd=str(self.work),
                    body=body,
                    request_id="root-" + nonce,
                )
            )
        self.save(
            "checkpoint", dict(roots=self.roots, initial_bodies=self.initial, work=str(self.work))
        )
        for result, nonce in zip(self.batch(name, operations), nonces, strict=True):
            self.roots[result["agent_id"]] = nonce
        self.pending_spawns.remove(name)
        self.save(
            "checkpoint", dict(roots=self.roots, initial_bodies=self.initial, work=str(self.work))
        )

    def observe(self):
        before = time.monotonic()
        agents = self.discover()
        after = time.monotonic()
        self.check(agents)
        self.observations.append(
            dict(
                before=before,
                after=after,
                agents=len(agents),
                idle=sum(a["state"] == "idle" for a in agents),
                stopped=sum(a["state"] == "stopped" for a in agents),
                completed=[a["id"] for a in agents if a["task_outcome"] == "completed"],
            )
        )
        return agents

    def wait_ready(self, count):
        while True:
            agents = self.observe()
            require(len(agents) <= count, "Unexpected extra agent")
            require(
                not any(a["state"] == "stopped" or a["task_outcome"] != "open" for a in agents),
                "An agent ended before START; retain protocol evidence",
            )
            ids = [a["id"] for a in agents]
            complete = 0
            if ids:
                early = self.database.execute(
                    "SELECT count(*) FROM messages WHERE startup=0 AND recipient IN ("
                    + ",".join("?" for _ in ids) + ")", ids
                ).fetchone()[0]
                require(early == 0, "A message arrived before START; retain protocol evidence")
                query = "SELECT count(*) FROM messages WHERE startup=1 AND state='completed' AND recipient IN ("
                complete = self.database.execute(
                    query + ",".join("?" for _ in ids) + ")", ids
                ).fetchone()[0]
            if len(agents) == complete == count and all(
                a["state"] == "idle" and a["turn_id"] is None and a["thread_id"] for a in agents
            ):
                return
            pause(0.5)

    def preflight_tools(self):
        child = next(a for a in self.discover() if a["role"] == "live-pong")
        events = event_pages(self.call, child["id"])
        completed = [e for e in events if e["type"] == "tool_completed"]
        require(len(completed) == 1, "Preflight must complete one read-only tool")
        item = completed[0]["data"]["item"]
        if self.providers["live-pong"] == "claude":
            result = item["result"]
            require(not result.get("is_error", False), "Claude preflight tool failed")
            content = result["content"]
        else:
            require(
                item["type"] == "dynamicToolCall" and item["success"], "Codex preflight tool failed"
            )
            content = item["contentItems"]
        texts = (
            [content]
            if isinstance(content, str)
            else [b["text"] for b in content if b["type"] in ("text", "inputText")]
        )
        require(
            len(texts) == 1 and isinstance(json.loads(texts[0])["agents"], list),
            "Preflight did not return the Hive agent list",
        )
        self.save("preflight-tool", completed[0])

    def reject_dead_end(self, agents):
        # One read snapshot: a tool error can finish a turn without completing the
        # task. Such an idle graph cannot progress and must not burn the deadline.
        ids = [agent["id"] for agent in agents]
        require(bool(ids), "Missing agents in pair observation")
        placeholders = ",".join("?" for _ in ids)
        active, pending, unfinished = self.database.execute(
            "SELECT "
            f"(SELECT count(*) FROM agents WHERE id IN ({placeholders}) AND "
            "(state NOT IN ('idle','stopped') OR turn_id IS NOT NULL OR control IS NOT NULL "
            "OR completion_pending IS NOT NULL "
            "OR (state<>'stopped' AND (task_outcome<>'open' OR expired=1)))), "
            "(SELECT count(*) FROM messages m JOIN agents a ON a.id=m.recipient "
            f"WHERE a.id IN ({placeholders}) AND m.state IN ('accepted','submitting','submitted')), "
            f"(SELECT count(*) FROM agents WHERE id IN ({placeholders}) AND state<>'stopped')",
            ids * 3,
        ).fetchone()
        require(
            not unfinished or active or pending,
            "Unfinished pairs have no active turn or pending message",
        )

    def capture(self, prefix=""):
        agents = self.discover()
        events = {a["id"]: event_pages(self.call, a["id"]) for a in agents}
        # Read this run's messages exactly once, without duplicate mailbox copies.
        ids = [a["id"] for a in agents]
        self.database.row_factory = sqlite3.Row
        messages = (
            [
                dict(row)
                for row in self.database.execute(
                    "SELECT * FROM messages WHERE recipient IN ("
                    + ",".join("?" for _ in ids)
                    + ")",
                    ids,
                )
            ]
            if ids
            else []
        )
        requests = (
            [
                dict(
                    sender=row["sender"],
                    id=row["id"],
                    body=json.loads(row["body"]),
                    response=json.loads(row["response"]),
                )
                for row in self.database.execute(
                    "SELECT sender,id,body,response FROM requests WHERE sender IN ("
                    + ",".join("?" for _ in ids)
                    + ")",
                    ids,
                )
            ]
            if ids
            else []
        )
        evidence = dict(
            agents=agents,
            events=events,
            messages=messages,
            requests=requests,
            roots=self.roots,
            initial_bodies=self.initial,
            expected_roles=self.expected,
        )
        self.save(prefix + "evidence", evidence)
        self.save("observations", self.observations)
        self.save("timings", self.timings)
        return evidence

    def cleanup(self):
        self.timings["cleanup_started"] = time.monotonic()

        def cancel():
            if self.daemon:
                errors = cancel_descendants(self.call, next(iter(self.roots), ""), self.discover)
                self.save("cleanup-errors", errors)
                require(not errors, "Cleanup incomplete; inspect retained state")
                verify_tasks(self.call)

        def capture():
            if self.database:
                self.capture()

        def stop_sampler():
            if self.sampler:
                self.sampler.stop()
                require(not self.sampler.failure, self.sampler.failure)

        def close():
            if self.database:
                self.database.close()

        def stop_daemon():
            if self.daemon:
                if self.sampler:
                    self.sampler.expect_shutdown = True
                self.daemon_owner.close()
                if self.sampler:
                    self.sampler.verify_gone()
                self.timings["daemon_stopped"] = time.monotonic()

        def record_timings():
            self.timings["cleanup_finished"] = time.monotonic()
            self.save("timings", self.timings)

        def hosted():
            with call_deadline(time.monotonic() + 90):
                cleanup_all(cancel, capture)

        with finalizer(record_timings):
            cleanup_all(hosted, close, stop_daemon, stop_sampler)
        if self.pending_spawns:
            raise RuntimeError(
                "Spawn acceptance remains unresolved; retain its exact intent and admission"
            )
        if self.admission is not None:
            self.admission.advance("cleaned")

    def run(self):
        with finalizer(self.cleanup):
            with phase("daemon"):
                self.start()
            with phase(PHASES["preflight"]):
                self.spawn([0], "preflight-spawn")
                self.wait_ready(2)
                self.preflight_tools()
                self.timings["preflight_ready"] = time.monotonic()
            with phase(PHASES["startup"]):
                if self.args.pairs > 1:
                    self.spawn(range(1, self.args.pairs), "remaining-spawn")
                self.wait_ready(2 * self.args.pairs)
                self.timings["all_ready"] = time.monotonic()
            with phase(PHASES["resident_hold"]):
                begin = time.monotonic()
                while time.monotonic() - begin < 10:
                    self.observe()
                    pause(0.5)
                self.timings["hold"] = [begin, time.monotonic()]
            with phase(PHASES["exchange"]):
                self.batch(
                    "release",
                    [
                        dict(
                            op="send",
                            target=aid,
                            body="START " + nonce,
                            request_id="start-" + nonce,
                        )
                        for aid, nonce in self.roots.items()
                    ],
                )
                while True:
                    agents = self.observe()
                    if all(a["state"] == "stopped" for a in agents):
                        break
                    self.reject_dead_end(agents)
                    pause(0.25)
                self.timings["all_stopped"] = time.monotonic()
            with phase("verify"):
                self.timings["verification_started"] = time.monotonic()
                evidence = self.capture()
                result = verify(evidence)
                self.success = True
                self.timings["verification_finished"] = time.monotonic()
        memory = summarize(self.output / "memory.jsonl", evidence, self.timings["hold"])
        self.save("memory-summary", memory)

        def accepted():
            self.save("verification", result)
            self.save("summary", dict(verification=result, memory=memory, timings=self.elapsed()))
            print(json.dumps(dict(result="passed", output=str(self.output))), flush=True)

        finish(accepted)

    def elapsed(self):
        t = self.timings
        completion = {}
        previous = t["release"][0]
        for sample in self.observations:
            if sample["after"] < t["release"][0]:
                continue
            for aid in sample["completed"]:
                completion.setdefault(
                    aid, [max(0, previous - t["release"][1]), sample["after"] - t["release"][0]]
                )
            previous = sample["before"]
        require(set(completion) == set(self.roots), "Missing completion observations")
        return dict(
            daemon_startup_seconds=t["daemon_ready"] - self.began,
            remaining_startup_seconds=t["all_ready"] - t["preflight_ready"],
            verification_seconds=t["verification_finished"] - t["verification_started"],
            cleanup_seconds=t["cleanup_finished"] - t["cleanup_started"],
            total_seconds=time.monotonic() - self.began,
            preflight_seconds=t["preflight_ready"] - t["preflight-spawn"][0],
            startup_seconds=t["all_ready"] - t["preflight-spawn"][0],
            hold_seconds=t["hold"][1] - t["hold"][0],
            exchange_and_shutdown_seconds=t["all_stopped"] - t["release"][0],
            run_to_all_stopped_seconds=t["all_stopped"] - t["preflight-spawn"][0],
            completion_lower_seconds=distribution([v[0] for v in completion.values()]),
            completion_upper_seconds=distribution([v[1] for v in completion.values()]),
            pair_completion_brackets=completion,
        )


def main():
    args = parse_args()
    with campaign(args, "live-pairs") as admission, deadline(args.timeout):
        Run(args, admission).run()


if __name__ == "__main__":
    raise SystemExit(entry(main, name="scenario"))
