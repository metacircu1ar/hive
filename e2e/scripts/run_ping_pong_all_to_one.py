#!/usr/bin/env python3
"""One coordinator spawns ping children and proves every pong was received."""

from release.files import atomic_write_text, atomic_write_json, create_directory

import argparse
import json
import time
import uuid
from pathlib import Path

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
    preflight,
    verify_tasks,
)
from release.cancellation import deadline, finish, pause
from release.progress import cleanup_all, finalizer

from e2e.scripts.harness.checks import one as require_one
from e2e.scripts.harness.checks import require
from e2e.scripts.harness.cli import call_deadline, event_pages, read_agents


def verify(evidence):
    return LiveAllToOneEvidence(evidence).verify()


class LiveAllToOneEvidence:
    """Check the fixed live graph using its recorded model and causality evidence."""

    def __init__(self, evidence):
        self.evidence = evidence
        self.root = self.evidence["root_id"]
        self.agents = self.evidence["agents"]
        self.events = self.evidence["events"]
        self.messages = self.evidence["messages"]
        self.children = self.evidence["expected_children"]
        require(
            type(self.children) is int and 1 <= self.children <= 10, "Invalid expected child count"
        )
        pong = next(a for a in self.agents if a["id"] == self.root)
        children = [a for a in self.agents if a["parent_id"] == self.root]
        require(
            len(self.agents) == self.children + 1 and len(children) == self.children,
            "Wrong direct child count",
        )
        require(pong["parent_id"] is None and pong["role"] == "pong", "Invalid root agent")
        require(pong["task_outcome"] == "completed", "Pong did not complete")
        require(
            len({a["thread_id"] for a in self.agents}) == self.children + 1,
            "Conversations were shared",
        )

    def of(self, agent_id, kind):
        return [e for e in self.events[agent_id] if e["type"] == kind]

    one = staticmethod(require_one)

    def verify(self):
        self.completed = self.one(self.of(self.root, "task_completed"), "pong completion")
        self.completion_request = self.one(
            self.of(self.root, "completion_requested"), "pong completion request"
        )
        require(self.completion_request["data"]["by"] == self.root, "Pong must complete itself")
        require(
            self.completion_request["seq"] < self.completed["seq"], "Invalid completion ordering"
        )
        self.all_receipts = []
        self.cancellations = []
        self.expected_messages = set()
        for agent in self.agents:
            self.agent(agent)
        return self.result()

    def agent(self, a):
        aid = a["id"]
        expected = self.evidence["expected_roles"][a["role"]]
        model, effort = expected["model"], expected["effort"]
        require(
            a["state"] == "stopped" and a["turn_id"] is None and a["control"] is None,
            f"{aid}: provider still active",
        )
        require(not a["expired"] and a["error"] is None, f"{aid}: expired or failed")
        require(a["model"] == model and a["effort"] == effort, f"{aid}: wrong configured model")
        ready = self.of(aid, "session_ready")
        require(
            ready
            and all(
                e["data"]["model"] == model
                and e["data"]["effort"] == effort
                and e["data"]["thread_id"] == a["thread_id"]
                for e in ready
            ),
            f"{aid}: wrong actual model or conversation",
        )
        require(
            not self.of(aid, "execution_failed") and not self.of(aid, "model_rerouted"),
            f"{aid}: execution failed or model changed",
        )
        starts = self.of(aid, "execution_starting")
        require(bool(starts), f"{aid}: missing execution")
        for start in starts:
            execution = start["execution_id"]
            released = [
                e
                for e in self.of(aid, "provider_session_released")
                if e["execution_id"] == execution
            ]
            if not released:
                released = [
                    e for e in self.of(aid, "provider_shutdown") if e["execution_id"] == execution
                ]
            receipt = self.one(released, f"{aid} conversation release")
            stopped = self.one(
                [e for e in self.of(aid, "execution_stopped") if e["execution_id"] == execution],
                f"{aid} execution stop",
            )
            require(
                receipt["data"]["confirmed"] is True
                and receipt["data"]["quiescent"] is True
                and start["seq"] < receipt["seq"] < stopped["seq"],
                f"{aid}: unconfirmed conversation release",
            )
        self.one(self.of(aid, "spawned"), f"{aid} spawn")
        if aid == self.root:
            return
        self.child(a, aid)

    def child(self, a, aid):
        require(
            a["role"] == "ping"
            and a["lifetime"] == "independent"
            and a["task_outcome"] == "cancelled",
            f"{aid}: not explicitly cancelled ping",
        )
        require(
            not self.of(aid, "completion_requested")
            and not self.of(aid, "task_completed")
            and not self.of(aid, "lifetime_expired"),
            f"{aid}: wrong termination mechanism",
        )
        cancelled = self.one(self.of(aid, "termination_requested"), f"{aid} termination")
        require(cancelled["data"]["by"] == self.root, f"{aid}: not terminated by pong")
        self.cancellations.append(cancelled["seq"])
        spawned = self.one(self.of(aid, "spawned"), f"{aid} spawn")
        require(spawned["data"]["parent_id"] == self.root, f"{aid}: not spawned by pong")
        startup = self.one(
            [m for m in self.messages if m["id"] == spawned["data"]["initial_message_id"]],
            f"{aid} startup message",
        )
        require(
            startup["sender"] == self.root
            and startup["recipient"] == aid
            and startup["state"] == "completed",
            f"{aid}: invalid startup",
        )
        self.expected_messages.add(startup["id"])
        ping, reply, ack = self.exchange(aid, startup)
        self.receipt(aid, cancelled, ping, reply, ack)

    def exchange(self, aid, startup):
        ping = self.one(
            [
                m
                for m in self.messages
                if m["sender"] == aid and m["recipient"] == self.root and m["body"] == "ping"
            ],
            f"{aid} ping",
        )
        reply = self.one(
            [
                m
                for m in self.messages
                if m["sender"] == self.root and m["recipient"] == aid and m["body"] == "pong"
            ],
            f"{aid} pong",
        )
        ack = self.one(
            [
                m
                for m in self.messages
                if m["sender"] == aid
                and m["recipient"] == self.root
                and m["body"] == "received pong"
            ],
            f"{aid} receipt",
        )
        self.expected_messages.update(m["id"] for m in (ping, reply, ack))
        for sent, triggering, recipient in ((ping, startup, self.root), (reply, ping, aid)):
            source_turn = self.one(
                [
                    e
                    for e in self.of(sent["sender"], "turn_started")
                    if e["data"]["message_id"] == triggering["id"]
                ],
                "sending turn",
            )
            acceptance = self.one(
                [
                    e
                    for e in self.of(recipient, "message_accepted")
                    if e["data"]["id"] == sent["id"]
                ],
                "message acceptance",
            )
            require(
                acceptance["data"]["sender_turn_id"] == source_turn["data"]["turn_id"]
                and source_turn["seq"] < acceptance["seq"],
                "Message sent from wrong turn",
            )
        return ping, reply, ack

    def receipt(self, aid, cancelled, ping, reply, ack):
        require(ping["state"] == ack["state"] == "completed", f"{aid}: undelivered ping/receipt")
        require(reply["state"] in ("completed", "interrupted"), f"{aid}: undelivered pong")
        delivery = self.one(
            [e for e in self.of(aid, "turn_started") if e["data"]["message_id"] == reply["id"]],
            f"{aid} pong delivery",
        )
        accepted_ack = self.one(
            [e for e in self.of(self.root, "message_accepted") if e["data"]["id"] == ack["id"]],
            f"{aid} accepted receipt",
        )
        require(
            accepted_ack["data"]["sender"] == aid
            and accepted_ack["data"]["sender_turn_id"] == delivery["data"]["turn_id"]
            and delivery["seq"] < accepted_ack["seq"] < cancelled["seq"],
            f"{aid}: receipt not sent from its pong turn before cancellation",
        )
        processed_ack = self.one(
            [e for e in self.of(self.root, "turn_started") if e["data"]["message_id"] == ack["id"]],
            f"{aid} processed receipt",
        )
        self.all_receipts.append(processed_ack["seq"])
        finished = self.one(
            [e for e in self.of(aid, "turn_finished") if e["data"]["message_id"] == reply["id"]],
            f"{aid} pong turn end",
        )
        require(finished["data"]["status"] == reply["state"], f"{aid}: inconsistent pong state")
        if reply["state"] == "interrupted":
            require(cancelled["seq"] < finished["seq"], f"{aid}: interrupted before cancellation")

    def result(self):
        initial = self.one(
            [m for m in self.messages if m["sender"] == "human" and m["recipient"] == self.root],
            "root task",
        )
        require(initial["state"] == "completed", "Initial root turn did not complete")
        self.expected_messages.add(initial["id"])
        require(
            {m["id"] for m in self.messages} == self.expected_messages,
            "Unexpected messages or participants",
        )
        require(
            max(self.all_receipts) < min(self.cancellations)
            and max(self.cancellations) < self.completion_request["seq"],
            "Terminated before all receipts or too late",
        )
        return {
            "result": "passed",
            "ping_agents": self.children,
            "pings": self.children,
            "pongs": self.children,
            "receipts": self.children,
            "explicit_cancellations": self.children,
            "confirmed_agent_shutdowns": self.children + 1,
        }


def parse_args():
    project = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=product_default("target/release/hivectl"))
    parser.add_argument("--config", type=Path)
    parser.add_argument("--state-dir", type=Path, default=default_state())
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--live", action="store_true")
    parser.add_argument("--provider", choices=("codex", "claude"), default="codex")
    parser.add_argument("--children", type=int, choices=range(1, 11), default=2)
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--expected-model")
    parser.add_argument("--expected-effort")
    args = parser.parse_args()
    if not 1 <= args.timeout <= 600:
        parser.error("Use a timeout of 1..600 seconds")
    args.expected_model = args.expected_model or (
        LIVE_CODEX_MODEL if args.provider == "codex" else LIVE_CLAUDE_MODEL
    )
    args.expected_effort = args.expected_effort or (LIVE_CODEX_EFFORT if args.provider == "codex" else "low")
    args.output_dir = args.output_dir or output_directory("live-all-to-one")
    opt_in(
        parser,
        args,
        agents=args.children + 1,
        models=dict(model=args.expected_model, effort=args.expected_effort),
    )
    require_paths(args, parser, "binary")
    return args, project


def observe(call, root, args):
    until = time.monotonic() + args.timeout
    last = None
    while True:
        all_agents = read_agents(call)
        ids = {root}
        while True:
            expanded = ids | {a["id"] for a in all_agents if a["parent_id"] in ids}
            if expanded == ids:
                break
            ids = expanded
        agents = [a for a in all_agents if a["id"] in ids]
        pong = require_one([a for a in agents if a["id"] == root], "coordinator")
        progress = (
            len(agents) - 1,
            sum(a["task_outcome"] == "cancelled" for a in agents),
            sum(a["state"] == "stopped" for a in agents),
            pong["task_outcome"],
        )
        if progress != last:
            print(
                json.dumps(
                    dict(zip(("children", "cancelled", "stopped", "pong_outcome"), progress))
                ),
                flush=True,
            )
            last = progress
        if any(a["state"] == "failed" or a["error"] is not None for a in agents):
            return agents, "Agent failed; retained evidence requires inspection"
        if time.monotonic() >= until:
            return agents, "Timed out; scenario agents will be terminated"
        if pong["task_outcome"] == "completed" and all(a["state"] == "stopped" for a in agents):
            return agents, None
        pause(0.25)


def write_evidence(args, root, agents, call, output, *, name="evidence.json"):
    evidence = dict(
        expected_children=args.children,
        expected_roles={
            role: dict(model=args.expected_model, effort=args.expected_effort)
            for role in ("pong", "ping")
        },
        root_id=root,
        agents=agents,
        messages=call("messages", root)["messages"] if root else [],
        events={a["id"]: event_pages(call, a["id"]) for a in agents},
    )
    atomic_write_json(output / name, evidence)
    return evidence


def collect_evidence(args, root, agents, call, output, failure):
    evidence = write_evidence(args, root, agents, call, output)
    require(failure is None, failure)
    return verify(evidence)


def spawn_root(call, output, work, body, *, on_submit=lambda: None):
    intent = output / "spawn-intent.json"
    require(not intent.exists(), "Unresolved spawn intent; inspect its exact request ID")
    request = uuid.uuid4().hex
    command = ("spawn", "pong", "--cwd", str(work), "--body", body, "--request-id", request)
    atomic_write_json(
        intent, dict(command=call.argv(*command), request_id=request), create_once=True
    )
    on_submit()
    return call(*command)["agent_id"]


def main():
    args, project = parse_args()
    output = args.output_dir.resolve()
    require(not output.exists(), "Use a fresh output directory")
    create_directory(output, mode=0o700)
    work = output / "work"
    create_directory(work, mode=0o700)
    roles = {}
    for role in ("ping", "pong"):
        instructions = (
            (project / "examples/roles" / (role + ".md"))
            .read_text()
            .replace("ten", str(args.children))
            .replace("ping-10", f"ping-{args.children:02}")
        )
        path = output / (role + ".md")
        atomic_write_text(path, instructions)
        roles[role] = dict(
            auth=args.provider + "-main",
            model=args.expected_model,
            effort=args.expected_effort,
            instructions=str(path),
            lifetime="independent",
            sandbox="read-only",
            api_fallback=False,
        )
    config = args.config.resolve() if args.config else output / "config.json"
    if args.config is None:
        atomic_write_json(
            config,
            dict(
                roles=roles,
                auth_profiles={
                    p + "-main": dict(provider=p, method="subscription")
                    for p in ("codex", "claude")
                },
            ),
        )
    remote = [str(args.binary), "--state-dir", str(args.state_dir.resolve())]
    call = configured_cli(remote, config)
    owner = Daemon(call, args.state_dir.resolve(), config, output)
    root = ""
    unresolved = False

    def submitted():
        nonlocal unresolved
        unresolved = True

    def discover():
        return owned_agents(call, work, [root] if root else [])

    def retire():
        if owner.ready:
            with finalizer(owner.close):
                with call_deadline(time.monotonic() + 90):
                    cleanup_all(
                        lambda: cancel_descendants(call, root, discover),
                        lambda: verify_tasks(call),
                        lambda: write_evidence(
                            args, root, discover(), call, output, name="cleanup-evidence.json"
                        ),
                    )
        else:
            owner.close()
        if unresolved:
            raise RuntimeError(
                "Spawn acceptance remains unresolved; retain its exact intent and admission"
            )
        admission.advance("cleaned")

    with campaign(args, "live-all-to-one") as admission, deadline(args.timeout):
        preflight(args.state_dir.resolve())
        require(
            call("auth", "status", args.provider + "-main")["state"] == "authenticated",
            "An existing authenticated login is required",
        )
        with finalizer(retire):
            owner.start(admission)
            task = (
                (project / "examples/ping-pong-all-to-one-task.md")
                .read_text()
                .replace("ten", str(args.children))
            )
            root = spawn_root(call, output, work, task, on_submit=submitted)
            unresolved = False
            atomic_write_json(output / "checkpoint.json", dict(root_id=root))
            agents, failure = observe(call, root, args)
            result = collect_evidence(args, root, agents, call, output, failure)
        finish(lambda: atomic_write_json(output / "verification.json", result))


if __name__ == "__main__":
    from release.progress import entry

    raise SystemExit(entry(main, name="scenario"))
