#!/usr/bin/env python3
"""Run 100 isolated commit reviews with 101 live agents and sampled process RSS.

Requires an existing Codex subscription login. The harness prepares the fixture,
starts only the implementor, measures/observes, and verifies. Model agents do all
review coordination and fixes. On failure, graceful cleanup cancels this run's
agents and saves failure evidence; it never replays uncertain work.
"""

from release.files import atomic_write_text

import argparse
import json
import time
import uuid
from functools import partial
from pathlib import Path

from e2e.live.scenario import Hosted, checked, options
from release.cancellation import pause, finish
from release.files import create_directory, publish_prepared

from e2e.scripts.commit_review_fixture import create, preflight, verify_repo, load_manifest
from e2e.scripts.harness.checks import one as require_one
from e2e.scripts.harness.checks import require
from release.progress import finalizer
from e2e.scripts.product_constants import LIVE_CODEX_EFFORT, LIVE_CODEX_MODEL, LIVE_REASONING_EFFORT
from e2e.scripts.scenario_memory import Sampler, summarize

one = partial(require_one, count_label="found")


def read_messages(call, root):
    messages, cursor = [], None
    while True:
        arguments = ["messages", root, "--limit", "200"]
        if cursor is not None:
            arguments += ["--before", str(cursor)]
        batch = call(*arguments)
        if not batch["messages"]:
            return messages
        messages += batch["messages"]
        next_cursor = batch["next_before"]
        require(
            next_cursor is not None and (cursor is None or next_cursor < cursor),
            "Review mailbox cursor did not advance",
        )
        cursor = next_cursor


def quiescent(agents, messages):
    # turn/start may be in flight while an agent still appears idle. Its inbox
    # entry is already submitting, so state alone is insufficient evidence.
    return all(a["state"] == "idle" for a in agents) and not any(
        m["state"] in ("accepted", "submitting", "submitted") for m in messages
    )


def verify(evidence, manifest, checks):
    return ReviewEvidence(evidence, manifest, checks).verify()


class ReviewEvidence:
    """Validate every review exchange against immutable assignments and final bytes."""

    def __init__(self, evidence, manifest, checks):
        self.root = evidence["root_id"]
        self.agents, self.events, self.messages = (
            evidence["agents"],
            evidence["events"],
            evidence["messages"],
        )
        parent = one([a for a in self.agents if a["id"] == self.root], "implementor")
        children = [a for a in self.agents if a["parent_id"] == self.root]
        require(
            len(self.agents) == 101 and len(children) == 100,
            "Expected 101 agents and 100 direct reviewers",
        )
        require(
            parent["role"] == "commit-implementor" and parent["task_outcome"] == "completed",
            "Implementor not complete",
        )
        require(len({a["thread_id"] for a in self.agents}) == 101, "Shared provider conversation")
        self.final_hashes = {c["id"]: c["sha256"] for c in checks}
        self.assigned = {a["id"]: a for a in manifest["assignments"]}
        require(len(self.assigned) == 100, "Assignment count mismatch")

    def of(self, aid, kind):
        return [e for e in self.events[aid] if e["type"] == kind]

    def turn(self, aid, mid):
        return one(
            [e for e in self.of(aid, "turn_started") if e["data"]["message_id"] == mid],
            "message turn",
        )

    def sent(self, message, trigger):
        origin = self.turn(message["sender"], trigger["id"])
        accepted = one(
            [
                e
                for e in self.of(message["recipient"], "message_accepted")
                if e["data"]["id"] == message["id"]
            ],
            "acceptance",
        )
        require(
            accepted["data"]["sender_turn_id"] == origin["data"]["turn_id"]
            and origin["seq"] < accepted["seq"],
            "Incorrect sending-turn provenance",
        )

    def verify(self):
        self.used, self.approved, self.cancelled, self.anomalies = set(), [], [], []
        self.known_messages = set()
        initial = one([m for m in self.messages if m["sender"] == "human"], "human task")
        require(
            initial["recipient"] == self.root and initial["state"] == "completed",
            "Invalid initial task",
        )
        self.known_messages.add(initial["id"])
        for agent in self.agents:
            self.agent(agent)
        return self.result()

    def agent(self, a):
        aid = a["id"]
        model, effort = (
            ("gpt-6.1-terra" if aid == self.root else LIVE_CODEX_MODEL),
            LIVE_REASONING_EFFORT if aid == self.root else LIVE_CODEX_EFFORT,
        )
        require(
            a["state"] == "stopped"
            and not a["expired"]
            and a["error"] is None
            and a["turn_id"] is None
            and a["control"] is None,
            f"{aid}: incomplete shutdown",
        )
        ready = one(self.of(aid, "session_ready"), "session")
        require(
            a["model"] == ready["data"]["model"] == model
            and a["effort"] == ready["data"]["effort"] == effort
            and a["thread_id"] == ready["data"]["thread_id"],
            f"{aid}: wrong model/conversation",
        )
        shutdown = one(self.of(aid, "provider_shutdown"), "shutdown")
        starting = one(self.of(aid, "execution_starting"), "execution")
        require(
            shutdown["execution_id"] == starting["execution_id"] and shutdown["data"]["confirmed"],
            "Unconfirmed execution cleanup",
        )
        require(
            not self.of(aid, "execution_failed")
            and not self.of(aid, "model_rerouted")
            and not self.of(aid, "control_requested"),
            "Failure, reroute or manual intervention",
        )
        if aid == self.root:
            return
        ReviewerEvidence(self, a, aid, shutdown).verify()

    def result(self):
        require(
            self.used == set(self.assigned)
            and self.known_messages == {m["id"] for m in self.messages},
            "Unexpected or missing messages",
        )
        completion = one(self.of(self.root, "completion_requested"), "completion request")
        finished = one(self.of(self.root, "task_completed"), "task completion")
        require(
            completion["data"]["by"] == self.root
            and max(self.approved) < min(self.cancelled)
            and max(self.cancelled) < completion["seq"] < finished["seq"],
            "Invalid acceptance/termination order",
        )
        return {
            "result": "passed",
            "commits": 100,
            "reviewers": 100,
            "agents": 101,
            "approved_final_files": 100,
            "messages": len(self.messages),
            "confirmed_shutdowns": 101,
            "corrected_verdict_anomalies": self.anomalies,
        }


class ReviewerEvidence:
    """One reviewer's ordered request/verdict state."""

    def __init__(self, evidence, a, aid, shutdown):
        self.evidence, self.aid, self.shutdown = evidence, aid, shutdown
        require(
            a["role"] == "commit-reviewer"
            and a["lifetime"] == "independent"
            and a["task_outcome"] == "cancelled",
            "Wrong reviewer lifecycle",
        )
        require(
            not self.evidence.of(self.aid, "completion_requested")
            and not self.evidence.of(self.aid, "lifetime_expired"),
            "Wrong termination mechanism",
        )
        spawn = one(self.evidence.of(self.aid, "spawned"), "reviewer spawn")
        self.assignment = json.loads(spawn["data"]["body"])["assignment"]
        self.key = self.assignment["id"]
        require(
            self.key not in self.evidence.used
            and self.assignment == self.evidence.assigned[self.key],
            "Missing/duplicate/changed assignment",
        )
        self.evidence.used.add(self.key)
        require(a["cwd"] == self.assignment["review_cwd"], "Reviewer did not get isolated checkout")
        self.exchange = sorted(
            [
                m
                for m in self.evidence.messages
                if {m["sender"], m["recipient"]} == {self.evidence.root, self.aid}
            ],
            key=lambda m: m["seq"],
        )
        require(len(self.exchange) >= 4, "No findings/fix/recheck cycle")
        require(
            self.exchange[0]["id"] == spawn["data"]["initial_message_id"], "Wrong initial review"
        )
        self.requests = [m for m in self.exchange if m["sender"] == self.evidence.root]
        self.replies = [m for m in self.exchange if m["sender"] == self.aid]
        self.groups = {
            self.evidence.turn(self.aid, m["id"])["data"]["turn_id"]: {"request": m, "replies": []}
            for m in self.requests
        }
        for reply in self.replies:
            accepted = one(
                [
                    e
                    for e in self.evidence.of(self.evidence.root, "message_accepted")
                    if e["data"]["id"] == reply["id"]
                ],
                "reply acceptance",
            )
            group = self.groups.get(accepted["data"]["sender_turn_id"])
            require(group is not None, "Reply without an assigned review turn")
            self.evidence.sent(reply, group["request"])
            self.evidence.turn(self.evidence.root, reply["id"])
            group["replies"].append(reply)
        for m in self.exchange:
            self.evidence.known_messages.add(m["id"])
            require(
                m["state"] == "completed" or (m in self.requests and m["state"] == "interrupted"),
                "Undelivered review input",
            )

    def verify(self):
        self.original = None
        self.original_replies = []
        self.final_reply = None
        self.interrupted = []
        self.unresolved_kind = None
        for group in self.groups.values():
            self.review_group(group)
        self.completion()

    def review_scope(self, group):
        request = group["request"]
        request_body = json.loads(request["body"])
        review = request_body == {"kind": "REVIEW", "assignment": self.assignment}
        recheck = request_body == {
            "kind": "RECHECK",
            "id": self.key,
            "commit": self.assignment["commit"],
            "path": self.assignment["working_file"],
        }
        require(review or recheck, "Wrong review/recheck scope")
        require(
            self.unresolved_kind is None or request_body["kind"] == self.unresolved_kind,
            "Malformed verdict requires a retry of the same review scope",
        )
        if request is not self.exchange[0]:
            acceptance = one(
                [
                    e
                    for e in self.evidence.of(self.aid, "message_accepted")
                    if e["data"]["id"] == request["id"]
                ],
                "request acceptance",
            )
            source = one(
                [
                    e
                    for e in self.evidence.of(self.evidence.root, "turn_started")
                    if e["data"]["turn_id"] == acceptance["data"]["sender_turn_id"]
                ],
                "request source turn",
            )
            trigger = one(
                [
                    m
                    for m in self.evidence.messages
                    if m["recipient"] == self.evidence.root
                    and m["id"] == source["data"]["message_id"]
                ],
                "implementor trigger",
            )
            self.evidence.sent(request, trigger)
        if recheck:
            require(
                self.original is not None
                and any(reply["seq"] < request["seq"] for reply in self.original),
                "Recheck before canonical original findings",
            )
            require(
                min(
                    self.evidence.turn(self.evidence.root, reply["id"])["seq"]
                    for reply in self.original_replies
                )
                < acceptance["seq"],
                "Recheck before implementor consumed original findings",
            )
        return request, request_body, review, recheck

    def verdict(self, group, recheck):
        require(group["replies"], "Review request has no verdict")
        resolved = None
        canonical_replies = []
        for reply in group["replies"]:
            try:
                body = json.loads(reply["body"])
            except (TypeError, ValueError):
                body = None
            canonical = (
                isinstance(body, dict)
                and body.get("id") == self.key
                and body.get("commit") == self.assignment["commit"]
                and isinstance(body.get("source_sha256"), str)
                and bool(body["source_sha256"])
                and isinstance(body.get("findings"), list)
                and (
                    (body.get("kind") == "FINDINGS" and bool(body["findings"]))
                    or (recheck and body.get("kind") == "NO_FINDINGS" and body["findings"] == [])
                )
                and (recheck or body["source_sha256"] == self.assignment["original_sha256"])
            )
            resolved = (reply, body) if canonical else None
            if canonical:
                canonical_replies.append(reply)
            if not canonical:
                self.evidence.anomalies.append(
                    {
                        "assignment": self.key,
                        "message_id": reply["id"],
                        "reason": "Noncanonical reviewer verdict; requires a later canonical correction",
                    }
                )
        return resolved, canonical_replies

    def review_group(self, group):
        request, request_body, review, recheck = self.review_scope(group)
        resolved, canonical_replies = self.verdict(group, recheck)
        # A later message in this input turn supersedes earlier replies. A
        # malformed last reply cannot silently inherit an earlier approval.
        if review and resolved:
            self.original = canonical_replies
            self.original_replies = group["replies"]
        self.unresolved_kind = None if resolved else request_body["kind"]
        self.final_reply = resolved if recheck else None
        if request["state"] == "interrupted":
            require(
                recheck
                and resolved
                and resolved[1]["kind"] == "NO_FINDINGS"
                and resolved[1]["source_sha256"] == self.evidence.final_hashes[self.key],
                "Interrupted request without final clean bytes",
            )
            self.interrupted.append((request, resolved[0]))

    def completion(self):
        require(self.original is not None, "No actual original findings")
        require(
            self.final_reply is not None
            and self.final_reply[1]["kind"] == "NO_FINDINGS"
            and self.final_reply[1]["source_sha256"] == self.evidence.final_hashes[self.key],
            "Final bytes not approved",
        )
        self.evidence.approved.append(
            self.evidence.turn(self.evidence.root, self.final_reply[0]["id"])["seq"]
        )
        termination = one(
            self.evidence.of(self.aid, "termination_requested"), "reviewer termination"
        )
        require(
            termination["data"]["by"] == self.evidence.root
            and termination["seq"] < self.shutdown["seq"],
            "Reviewer not shut down by parent",
        )
        for request, reply in self.interrupted:
            ending = one(
                [
                    e
                    for e in self.evidence.of(self.aid, "turn_finished")
                    if e["data"]["message_id"] == request["id"]
                ],
                "interrupted recheck end",
            )
            clean_sent = one(
                [
                    e
                    for e in self.evidence.of(self.evidence.root, "message_accepted")
                    if e["data"]["id"] == reply["id"]
                ],
                "clean reply acceptance",
            )
            require(
                clean_sent["seq"] < termination["seq"] < ending["seq"]
                and ending["data"]["status"] == "interrupted",
                "Invalid interrupted final recheck",
            )
        self.evidence.cancelled.append(termination["seq"])


def verify_saved(output, *, publish=True, announce=False):
    """Recheck frozen run evidence and current final bytes without running agents."""

    def verdict(value):
        if not publish:
            return
        atomic_write_text(
            output / "verification.json",
            json.dumps({**value, "verified_at_unix": time.time()}, indent=2) + "\n",
        )

    verdict({"result": "pending"})
    try:
        manifest = load_manifest(output)
        evidence = json.loads((output / "evidence.json").read_text())
        require(evidence["failure"] is None, evidence["failure"])
        checks = verify_repo(manifest)
        atomic_write_text(output / "independent-checks.json", json.dumps(checks, indent=2) + "\n")
        memory = summarize(output / "memory.jsonl", evidence["agents"], evidence["events"])
        atomic_write_text(output / "memory-summary.json", json.dumps(memory, indent=2) + "\n")
        # Preserve independently established correctness and resource measurements
        # even when the exchange fails protocol acceptance. Only the verdict is
        # authoritative for whole-scenario acceptance.
        result = verify(evidence, manifest, checks)
        failure = output / "failure.json"
        if failure.exists():
            publish_prepared(failure, output / f"failure-before-reverification-{uuid.uuid4()}.json")
    except BaseException as error:
        with finalizer(
            lambda failure=error: verdict({"result": "failed", "error": type(failure).__name__})
        ):
            raise
    report = {
        **result,
        "peak_rss_gib": memory["peak"]["total_rss_kib"] / 1024**2,
        "loaded101_peak_rss_gib": memory["all_agents_loaded_peak"]["total_rss_kib"] / 1024**2,
        "evidence": str(output / "evidence.json"),
    }

    if publish:

        def accepted():
            verdict(result)
            if announce:
                print(json.dumps(report), flush=True)

        finish(accepted)
    return report


def parse_args():
    project = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    options(parser, cap=14400)
    parser.add_argument("--prepare-only", action="store_true")
    parser.add_argument(
        "--verify-only", action="store_true", help="Verify saved evidence without provider work"
    )
    parser.add_argument("--idle-timeout", type=int, default=1800)
    parser.add_argument("--max-rss-gib", type=float, default=32)
    args = parser.parse_args()
    if args.idle_timeout < 1 or not 0 < args.max_rss_gib <= 32:
        parser.error("Use a positive idle deadline and RSS cap at most32GiB")
    if args.verify_only or args.prepare_only:
        if args.output_dir is None or args.verify_only and args.prepare_only:
            parser.error("Select one offline mode and an explicit --output-dir")
    else:
        checked(
            parser,
            args,
            label="commit-review",
            agents=101,
            models=dict(
                implementor=dict(model="gpt-6.1-terra", effort=LIVE_REASONING_EFFORT),
                reviewers=dict(model=LIVE_CODEX_MODEL, effort=LIVE_CODEX_EFFORT),
            ),
            cap=None,
        )
    return args, project


class CommitReviewRun(Hosted):
    """One 101-agent run, retaining its original isolated-process measurement."""

    def __init__(self, args, project):
        self.project = project
        roles = {
            name: dict(
                auth="codex-main",
                model="gpt-6.1-terra" if name == "commit-implementor" else LIVE_CODEX_MODEL,
                effort=LIVE_REASONING_EFFORT if name == "commit-implementor" else LIVE_CODEX_EFFORT,
                instructions=str(project / "examples/roles" / (name + ".md")),
                lifetime="independent",
                sandbox="workspace-write" if name == "commit-implementor" else "read-only",
            )
            for name in ("commit-implementor", "commit-reviewer")
        }
        # This explicit stress scenario measures101 separate processes. Everyday
        # live checks retain the shipping16-conversation process setting.
        super().__init__(
            args,
            label="commit-review",
            roles=roles,
            runtime=dict(
                sessions_per_codex_process=1,
                idle_seconds=min(86400, max(600, args.timeout)),
                warm_providers=0,
                linger_seconds=0,
            ),
        )
        self.call, self.root = self.cli, None
        self.sampler, self.failure, self.evidence = None, None, None
        self.agents = []
        self.manifest_path = self.output / "manifest.json"

    def prepare(self):
        self.manifest = create(self.output)
        require(len(self.manifest["assignments"]) == 100, "Expected 100 assignments")
        preflight(self.manifest)
        self.work = Path(self.manifest["repo"])

    def observe(self):
        self.sampler = Sampler(self.daemon["pid"], self.output, self.args.max_rss_gib, cpu=True)
        self.sampler.start()

    def stop_observers(self):
        if self.sampler is not None:
            self.sampler.expect_shutdown = True
            self.sampler.stop()

    def cleanup(self):
        if self.sampler is not None:
            self.sampler.expect_shutdown = True
        super().cleanup()

    def retain(self):
        if self.root is None:
            super().retain()
            return
        if self.evidence is None or self.failure is not None:
            self.collect()
        for agent in self.discover():
            atomic_write_text(
                self.output / (agent["id"] + "-status.json"), json.dumps(dict(agent=agent))
            )

    def descendants(self):
        return self.discover()

    def execute(self):
        try:
            task = (
                "Run the 100-commit review scenario in your role instructions. Immutable manifest: "
                + str(self.manifest_path)
                + ". Working repository: "
                + self.manifest["repo"]
                + ". Spawn all 100 assigned reviewers in the initial turn. Fix actual findings in the current "
                "uncommitted diff and get a clean recheck from every SAME reviewer before terminating them "
                "and completing yourself. Do not alter manifest, contracts, pinned review checkouts or Git "
                "history. Keep your ledger outside the repository."
            )
            self.root = self.spawn_task("commit-implementor", task)["agent_id"]
            self.wait()
            self.collect()
            require(self.evidence is not None, "Missing final review evidence")
            self.sampler.stop()
            return self.verify()
        except BaseException as error:
            self.failure = type(error).__name__
            raise

    def wait(self):
        self.began = time.monotonic()
        self.deadline = self.began + self.args.timeout
        self.last_activity = self.began
        self.next_events = self.began
        self.event_cursor = 0
        self.turns = 0
        self.last = None
        self.last_print = 0
        self.idle_since = None
        while True:
            self.agents = self.descendants()
            self.poll_events()
            parent = one([a for a in self.agents if a["id"] == self.root], "implementor")
            self.check_idle(parent)
            self.report_progress(parent)
            require(not self.sampler.failure, self.sampler.failure)
            require(not any(a["state"] == "failed" for a in self.agents), "Agent execution failed")
            require(time.monotonic() < self.deadline, "Scenario timed out")
            require(
                time.monotonic() - self.last_activity < self.args.idle_timeout,
                "Scenario stopped making observable progress",
            )
            if parent["task_outcome"] == "completed" and all(
                a["state"] == "stopped" for a in self.agents
            ):
                break
            pause(1)

    def poll_events(self):
        if time.monotonic() >= self.next_events:
            while True:
                batch = self.call(
                    "read", self.root, "--after", str(self.event_cursor), "--limit", "200"
                )
                require(not batch.get("history_expired"), "Review progress history expired")
                if not batch["events"]:
                    break
                require(batch["cursor"] > self.event_cursor, "Review event cursor did not advance")
                self.last_activity = time.monotonic()
                self.turns += sum(e["type"] == "turn_finished" for e in batch["events"])
                self.event_cursor = batch["cursor"]
            self.next_events = time.monotonic() + 5

    def check_idle(self, parent):
        if parent["task_outcome"] == "open" and all(a["state"] == "idle" for a in self.agents):
            if self.idle_since is None:
                self.idle_since = time.monotonic()
            elif (
                time.monotonic() - self.idle_since >= 60
                and time.monotonic() - self.last_activity >= 60
            ):
                # Every scenario message must involve the root. Read all
                # pages so old accepted/submitting inputs cannot be missed.
                messages = read_messages(self.call, self.root)
                require(
                    not quiescent(self.descendants(), messages),
                    "Scenario deadlocked: all agents idle for 60 seconds, no pending messages, task incomplete",
                )
                self.idle_since = time.monotonic()
        else:
            self.idle_since = None

    def report_progress(self, parent):
        state = (
            len(self.agents) - 1,
            sum(a["thread_id"] is not None for a in self.agents),
            sum(a["task_outcome"] == "cancelled" for a in self.agents),
            parent["task_outcome"],
        )
        if state != self.last or time.monotonic() - self.last_print > 30:
            print(
                json.dumps(
                    {
                        "reviewers": state[0],
                        "sessions": state[1],
                        "cancelled": state[2],
                        "implementor_outcome": state[3],
                        "rss_gib": round(
                            (self.sampler.latest or {}).get("total_rss_kib", 0) / 1024**2, 3
                        ),
                        "implementor_turns": self.turns,
                        "turns_per_hour": round(
                            self.turns * 3600 / max(1, time.monotonic() - self.began), 1
                        ),
                        "idle_seconds": round(time.monotonic() - self.last_activity),
                    }
                ),
                flush=True,
            )
            self.last, self.last_print = state, time.monotonic()

    def collect(self):
        if self.root:
            self.agents = self.descendants()
            self.evidence = dict(
                root_id=self.root,
                agents=self.agents,
                messages=read_messages(self.call, self.root),
                events={},
                failure=self.failure,
            )
            for agent in self.agents:
                self.evidence["events"][agent["id"]] = self.events(agent["id"])
            atomic_write_text(
                self.output / "evidence.json", json.dumps(self.evidence, indent=2) + "\n"
            )

    def verify(self):
        require(self.failure is None, "Review evidence collection failed")
        require(not self.sampler.failure, "Memory sampling failed")
        return verify_saved(self.output, publish=False)


def main():
    args, project = parse_args()
    if args.verify_only:
        verify_saved(args.output_dir.resolve(), announce=True)
        return
    if args.prepare_only:
        create_directory(args.output_dir, mode=0o700)
        manifest = create(args.output_dir)
        print(
            json.dumps(
                dict(
                    manifest=str(args.output_dir / "manifest.json"),
                    commits=len(manifest["commits"]),
                )
            ),
            flush=True,
        )
        return
    return CommitReviewRun(args, project).run()


if __name__ == "__main__":
    from release.progress import entry

    raise SystemExit(entry(main, name="scenario"))
