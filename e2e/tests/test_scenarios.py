from release.progress import entry
from e2e.live.ownership import cancel_descendants
from release.files import atomic_write_text
import copy
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from e2e.scripts import commit_review_fixture as corpus
from e2e.scripts.product_constants import LIVE_CODEX_EFFORT, LIVE_CODEX_MODEL, LIVE_REASONING_EFFORT
from e2e.scripts.run_commit_review_demo import (
    quiescent,
    read_messages,
    verify,
    verify_saved,
)
from e2e.scripts.scenario_memory import Sampler, summarize


def evidence_fixture():
    events = {"root": []}
    messages, agents, assignments, checks = [], [], [], []
    counter = 0

    def event(aid, kind, data):
        nonlocal counter
        counter += 1
        value = {
            "seq": counter,
            "timestamp": counter,
            "execution_id": "exec-" + aid,
            "type": kind,
            "data": data,
        }
        events[aid].append(value)
        return value

    def agent(aid, parent):
        review_agent(aid, parent, events, agents, event)

    def message(sender, recipient, body, source_turn=None):
        mid = "m" + str(len(messages) + 1)
        value = {
            "seq": len(messages) + 1,
            "id": mid,
            "sender": sender,
            "recipient": recipient,
            "body": json.dumps(body),
            "state": "completed",
        }
        messages.append(value)
        if source_turn:
            event(recipient, "message_accepted", {"id": mid, "sender_turn_id": source_turn})
        return value

    def turn(aid, m):
        tid = "turn-" + m["id"]
        event(aid, "turn_started", {"message_id": m["id"], "turn_id": tid})
        return tid

    agent("root", None)
    initial = message("human", "root", "task")
    turn("root", initial)
    for i in range(100):
        review_exchange(i, assignments, checks, agent, message, event, turn)
    for a in agents[1:]:
        event(a["id"], "termination_requested", {"by": "root"})
        event(a["id"], "provider_shutdown", {"confirmed": True})
    event("root", "completion_requested", {"by": "root"})
    event("root", "task_completed", {})
    event("root", "provider_shutdown", {"confirmed": True})
    return (
        {"root_id": "root", "agents": agents, "events": events, "messages": messages},
        {"assignments": assignments},
        checks,
    )


def review_agent(aid, parent, events, agents, event):
    events[aid] = []
    role = "commit-implementor" if parent is None else "commit-reviewer"
    model = "gpt-6.1-terra" if parent is None else LIVE_CODEX_MODEL
    effort = LIVE_REASONING_EFFORT if parent is None else LIVE_CODEX_EFFORT
    a = {
        "id": aid,
        "parent_id": parent,
        "role": role,
        "model": model,
        "effort": effort,
        "lifetime": "independent",
        "state": "stopped",
        "task_outcome": "completed" if parent is None else "cancelled",
        "expired": False,
        "error": None,
        "turn_id": None,
        "control": None,
        "thread_id": "thread-" + aid,
        "cwd": "/reviews/" + aid,
    }
    agents.append(a)
    event(aid, "execution_starting", {})
    event(aid, "session_ready", {"model": model, "effort": effort, "thread_id": a["thread_id"]})


def review_exchange(i, assignments, checks, agent, message, event, turn):
    aid = f"{i:03}"
    a = {
        "id": aid,
        "commit": "commit-" + aid,
        "module": aid + ".py",
        "working_file": "/repo/" + aid + ".py",
        "review_cwd": "/reviews/" + aid,
        "original_sha256": "old-" + aid,
    }
    assignments.append(a)
    checks.append({"id": aid, "sha256": "new-" + aid})
    agent(aid, "root")
    review = message("root", aid, {"kind": "REVIEW", "assignment": a})
    event(aid, "spawned", {"body": review["body"], "initial_message_id": review["id"]})
    t = turn(aid, review)
    finding = message(
        aid,
        "root",
        {
            "kind": "FINDINGS",
            "id": aid,
            "commit": a["commit"],
            "source_sha256": a["original_sha256"],
            "findings": ["counterexample"],
        },
        t,
    )
    t = turn("root", finding)
    recheck = message(
        "root",
        aid,
        {"kind": "RECHECK", "id": aid, "commit": a["commit"], "path": a["working_file"]},
        t,
    )
    t = turn(aid, recheck)
    clean = message(
        aid,
        "root",
        {
            "kind": "NO_FINDINGS",
            "id": aid,
            "commit": a["commit"],
            "source_sha256": "new-" + aid,
            "findings": [],
        },
        t,
    )
    turn("root", clean)


class ReviewVerifierTests(unittest.TestCase):
    def test_idle_agents_with_older_pending_or_submitting_input_are_not_deadlocked(self):
        agents = [{"state": "idle"} for _ in range(101)]
        called = []
        pages = [
            {"messages": [{"state": "completed"}] * 200, "next_before": 123},
            {"messages": [{"state": "submitting"}], "next_before": 122},
            {"messages": [], "next_before": None},
        ]

        def call(*args):
            called.append(args)
            return pages.pop(0)

        messages = read_messages(call, "root")
        self.assertEqual(called[1], ("messages", "root", "--limit", "200", "--before", "123"))
        self.assertFalse(quiescent(agents, messages))
        for state in ("accepted", "submitted"):
            messages[-1]["state"] = state
            self.assertFalse(quiescent(agents, messages))
        messages[-1]["state"] = "completed"
        self.assertTrue(quiescent(agents, messages))
        agents[-1]["state"] = "running"
        self.assertFalse(quiescent(agents, messages))

    def corrected_reply(self, evidence, index, replacement):
        """Add an explicitly corrected message in the same source input turn."""
        prior = evidence["messages"][index]
        correction = {
            **prior,
            "id": prior["id"] + "-correction",
            "seq": prior["seq"] + 0.25,
            "body": json.dumps(replacement),
        }
        evidence["messages"].insert(index + 1, correction)
        acceptance = next(
            e
            for e in evidence["events"]["root"]
            if e["type"] == "message_accepted" and e["data"]["id"] == prior["id"]
        )
        evidence["events"]["root"].append(
            {
                **acceptance,
                "seq": acceptance["seq"] + 0.1,
                "data": {**acceptance["data"], "id": correction["id"]},
            }
        )
        consumed = next(
            e
            for e in evidence["events"]["root"]
            if e["type"] == "turn_started" and e["data"]["message_id"] == prior["id"]
        )
        evidence["events"]["root"].append(
            {
                **consumed,
                "seq": consumed["seq"] + 0.1,
                "data": {"message_id": correction["id"], "turn_id": "turn-" + correction["id"]},
            }
        )
        return correction

    def test_corrections_are_bound_to_input_turn_and_exact_identity(self):
        e, m, c = evidence_fixture()
        good = json.loads(e["messages"][2]["body"])
        e["messages"][2]["body"] = json.dumps({**good, "source_sha256": "truncated"})
        with self.assertRaisesRegex(
            RuntimeError, "Malformed verdict requires a retry of the same review scope"
        ):
            verify(e, m, c)
        correction = self.corrected_reply(e, 2, good)
        result = verify(e, m, c)
        self.assertEqual(result["messages"], 402)
        self.assertEqual(len(result["corrected_verdict_anomalies"]), 1)
        correction["body"] = json.dumps({**good, "commit": "typo"})
        with self.assertRaisesRegex(
            RuntimeError, "Malformed verdict requires a retry of the same review scope"
        ):
            verify(e, m, c)
        correction["body"] = json.dumps(good)
        accepted = next(
            x
            for x in e["events"]["root"]
            if x["type"] == "message_accepted" and x["data"]["id"] == correction["id"]
        )
        accepted["data"]["sender_turn_id"] = "turn-from-another-request"
        with self.assertRaisesRegex(RuntimeError, "Reply without an assigned review turn"):
            verify(e, m, c)

    def test_trailing_malformed_verdict_does_not_inherit_prior_approval(self):
        e, m, c = evidence_fixture()
        clean = json.loads(e["messages"][4]["body"])
        self.corrected_reply(e, 4, {**clean, "commit": "typo"})
        with self.assertRaisesRegex(RuntimeError, "Final bytes not approved"):
            verify(e, m, c)
        e, m, c = evidence_fixture()
        clean = json.loads(e["messages"][4]["body"])
        self.corrected_reply(e, 4, {**clean, "kind": "FINDINGS", "findings": ["still broken"]})
        with self.assertRaisesRegex(RuntimeError, "Final bytes not approved"):
            verify(e, m, c)

    def test_revalidation_can_start_during_another_reviewers_turn(self):
        e, m, c = evidence_fixture()
        request = {**e["messages"][3], "id": "revalidate", "seq": 401.25}
        reply = {**e["messages"][4], "id": "revalidated", "seq": 401.5}
        e["messages"] += [request, reply]
        # Root retains its assignment ledger across turns. The last inbox item
        # (from reviewer099) may trigger a revalidation request for reviewer000.
        last = next(
            x
            for x in e["events"]["root"]
            if x["type"] == "turn_started" and x["data"]["message_id"] == "m401"
        )
        base = last["seq"]

        def event(aid, seq, kind, data):
            e["events"][aid].append({"seq": seq, "type": kind, "data": data})

        event(
            "000",
            base + 0.1,
            "message_accepted",
            {"id": request["id"], "sender_turn_id": last["data"]["turn_id"]},
        )
        event(
            "000",
            base + 0.2,
            "turn_started",
            {"message_id": request["id"], "turn_id": "revalidation-turn"},
        )
        event(
            "root",
            base + 0.3,
            "message_accepted",
            {"id": reply["id"], "sender_turn_id": "revalidation-turn"},
        )
        event(
            "root",
            base + 0.4,
            "turn_started",
            {"message_id": reply["id"], "turn_id": "consume-revalidation"},
        )
        self.assertEqual(verify(e, m, c)["messages"], 403)
        reply["state"] = "accepted"
        with self.assertRaisesRegex(RuntimeError, "Undelivered review input"):
            verify(e, m, c)
        reply["state"] = "completed"
        body = json.loads(reply["body"])
        body["source_sha256"] = "stale"
        reply["body"] = json.dumps(body)
        with self.assertRaisesRegex(RuntimeError, "Final bytes not approved"):
            verify(e, m, c)

    def test_duplicate_original_replies_do_not_require_alternation(self):
        e, m, c = evidence_fixture()
        good = json.loads(e["messages"][2]["body"])
        self.corrected_reply(e, 2, good)
        self.assertEqual(verify(e, m, c)["messages"], 402)

    def test_late_canonical_duplicate_preserves_earlier_original_evidence(self):
        e, m, c = evidence_fixture()
        good = json.loads(e["messages"][2]["body"])
        correction = self.corrected_reply(e, 2, good)
        correction["seq"] = 4.25  # The first RECHECK already exists.
        recheck_turn = next(
            x
            for x in e["events"]["000"]
            if x["type"] == "turn_started" and x["data"]["message_id"] == "m4"
        )
        accepted = next(
            x
            for x in e["events"]["root"]
            if x["type"] == "message_accepted" and x["data"]["id"] == correction["id"]
        )
        accepted["seq"] = recheck_turn["seq"] - 0.1
        consumed = next(
            x
            for x in e["events"]["root"]
            if x["type"] == "turn_started" and x["data"]["message_id"] == correction["id"]
        )
        consumed["seq"] = recheck_turn["seq"] + 0.1
        self.assertEqual(verify(e, m, c)["messages"], 402)
        e["messages"][2]["body"] = json.dumps({**good, "source_sha256": "bad"})
        with self.assertRaisesRegex(RuntimeError, "Recheck before canonical original findings"):
            verify(e, m, c)

    def test_saved_verdict_tracks_pass_fail_pass_and_preserves_prior_failure(self):
        e, m, c = evidence_fixture()
        e["failure"] = None
        memory = {"peak": {"total_rss_kib": 10}, "all_agents_loaded_peak": {"total_rss_kib": 10}}
        with (
            tempfile.TemporaryDirectory() as tmp,
            patch("e2e.scripts.run_commit_review_demo.verify_repo", return_value=c),
            patch("e2e.scripts.run_commit_review_demo.summarize", return_value=memory),
        ):
            output = Path(tmp)
            atomic_write_text(output / "baseline.json", "[]\n")
            m["baseline_sha256"] = corpus.digest(output / "baseline.json")
            atomic_write_text(output / "manifest.json", json.dumps(m))
            atomic_write_text(output / "evidence.json", json.dumps(e))
            self.assertEqual(entry(lambda: verify_saved(output), name="scenario"), 0)
            self.assertEqual(
                json.loads((output / "verification.json").read_text())["result"], "passed"
            )
            saved = copy.deepcopy(e)
            e["messages"][4]["state"] = "accepted"
            atomic_write_text(output / "evidence.json", json.dumps(e))
            (output / "independent-checks.json").unlink()
            (output / "memory-summary.json").unlink()
            with self.assertRaisesRegex(RuntimeError, "Undelivered review input"):
                verify_saved(output)
            self.assertEqual(
                json.loads((output / "verification.json").read_text())["result"], "failed"
            )
            self.assertEqual(json.loads((output / "independent-checks.json").read_text()), c)
            self.assertEqual(json.loads((output / "memory-summary.json").read_text()), memory)
            atomic_write_text(output / "failure.json", '{"error":"prior verifier failure"}')
            atomic_write_text(output / "evidence.json", json.dumps(saved))
            self.assertEqual(entry(lambda: verify_saved(output), name="scenario"), 0)
            self.assertEqual(
                json.loads((output / "verification.json").read_text())["result"], "passed"
            )
            self.assertFalse((output / "failure.json").exists())
            self.assertEqual(len(list(output.glob("failure-before-reverification-*.json"))), 1)

    def test_offline_verifier_cancellation_before_final_decision_never_passes(self):
        import os
        import signal
        import contextlib
        import io
        from release.cancellation import finish

        e, manifest, checks = evidence_fixture()
        e["failure"] = None
        memory = dict(peak=dict(total_rss_kib=10), all_agents_loaded_peak=dict(total_rss_kib=10))
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            atomic_write_text(output / "baseline.json", "[]\n")
            manifest["baseline_sha256"] = corpus.digest(output / "baseline.json")
            atomic_write_text(output / "manifest.json", json.dumps(manifest))
            atomic_write_text(output / "evidence.json", json.dumps(e))

            def cancelled(action):
                os.kill(os.getpid(), signal.SIGINT)
                return finish(action)

            with (
                patch("e2e.scripts.run_commit_review_demo.verify_repo", return_value=checks),
                patch("e2e.scripts.run_commit_review_demo.summarize", return_value=memory),
                patch("e2e.scripts.run_commit_review_demo.finish", side_effect=cancelled),
            ):
                with contextlib.redirect_stdout(io.StringIO()) as announced:
                    self.assertEqual(
                        entry(lambda: verify_saved(output, announce=True), name="scenario"), 130
                    )
                self.assertEqual(announced.getvalue(), "")
            self.assertEqual(
                json.loads((output / "verification.json").read_text())["result"], "pending"
            )

    def test_valid_and_corrupt_exchanges(self):
        original, manifest, checks = evidence_fixture()
        self.assertEqual(verify(original, manifest, checks)["messages"], 401)
        mutations = [
            lambda e: e["agents"].pop(),
            lambda e: e["agents"][1].update(model="wrong"),
            lambda e: e["agents"][1].update(cwd="/wrong"),
            lambda e: e["agents"][1].update(task_outcome="completed"),
            lambda e: e["messages"][2].update(state="accepted"),
            lambda e: e["messages"][4].update(
                body=json.dumps(
                    {
                        "kind": "NO_FINDINGS",
                        "id": "000",
                        "commit": "commit-000",
                        "source_sha256": "wrong",
                        "findings": [],
                    }
                )
            ),
            lambda e: next(x for x in e["events"]["root"] if x["type"] == "message_accepted")[
                "data"
            ].update(sender_turn_id="wrong"),
            lambda e: next(x for x in e["events"]["000"] if x["type"] == "termination_requested")[
                "data"
            ].update(by="human"),
            lambda e: next(x for x in e["events"]["000"] if x["type"] == "provider_shutdown")[
                "data"
            ].update(confirmed=False),
        ]
        diagnostics = [
            "Expected 101 agents and 100 direct reviewers",
            "wrong model/conversation",
            "Reviewer did not get isolated checkout",
            "Wrong reviewer lifecycle",
            "Undelivered review input",
            "Final bytes not approved",
            "Reply without an assigned review turn",
            "Reviewer not shut down by parent",
            "Unconfirmed execution cleanup",
        ]
        for mutate, expected in zip(mutations, diagnostics, strict=True):
            value = copy.deepcopy(original)
            mutate(value)
            with self.assertRaisesRegex(RuntimeError, expected):
                verify(value, manifest, checks)

    def test_final_recheck_interruption_requires_clean_reply_before_cancellation(self):
        e, m, c = evidence_fixture()
        recheck = e["messages"][3]
        recheck["state"] = "interrupted"
        cancelled = next(x for x in e["events"]["000"] if x["type"] == "termination_requested")[
            "seq"
        ]
        e["events"]["000"].append(
            {
                "seq": cancelled + 0.5,
                "type": "turn_finished",
                "data": {"message_id": recheck["id"], "status": "interrupted"},
            }
        )
        self.assertEqual(verify(e, m, c)["result"], "passed")
        e["events"]["000"][-1]["seq"] = 0
        with self.assertRaisesRegex(RuntimeError, "Invalid interrupted final recheck"):
            verify(e, m, c)

    def test_cleanup_continues_after_individual_error(self):
        agents = [
            dict(
                id=x,
                parent_id=None if x == "root" else "root",
                task_outcome="open",
                expired=False,
                state="running",
            )
            for x in ("root", "a", "b")
        ]
        attempts, failed = [], set()

        def call(op, aid):
            agent = next(a for a in agents if a["id"] == aid)
            if op == "status":
                return dict(agent=copy.deepcopy(agent))
            if op == "stop":
                agent["state"] = "stopped"
                return {}
            attempts.append(aid)
            if aid == "a" and aid not in failed:
                failed.add(aid)
                raise RuntimeError("Fixture transient termination failure")
            agent.update(task_outcome="cancelled", state="stopped")
            return {}

        with patch("e2e.live.ownership.pause"):
            self.assertEqual(cancel_descendants(call, "root", lambda: copy.deepcopy(agents)), [])
        self.assertEqual(attempts[:3], ["root", "a", "b"])
        self.assertEqual(attempts[-1], "a")


class OracleTests(unittest.TestCase):
    def test_early_exit_is_not_a_pass(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "candidate.py"
            for source in ("raise SystemExit(0)", "def calculate(n):\n    raise SystemExit(0)"):
                atomic_write_text(p, source)
                self.assertFalse(corpus.check(p, "factorial")["passed"])

    def test_stale_bytecode_cannot_change_tested_bytes(self):
        import py_compile

        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "candidate.py"
            atomic_write_text(
                p,
                "import math\ndef calculate(n):\n"
                "    if isinstance(n,bool) or not isinstance(n,int): raise TypeError()\n"
                "    if n<0: raise ValueError()\n    return math.factorial(n)\n",
            )
            self.assertTrue(corpus.check(p, "factorial")["passed"])
            py_compile.compile(
                str(p), invalidation_mode=py_compile.PycInvalidationMode.UNCHECKED_HASH
            )
            atomic_write_text(p, "def calculate(n):\n    return 123\n")
            self.assertFalse(corpus.check(p, "factorial")["passed"])

    def test_hundred_faulty_commits_and_clean_preflight(self):
        with tempfile.TemporaryDirectory() as tmp:
            m = corpus.create(Path(tmp))
            self.assertEqual(corpus.load_manifest(Path(tmp)), m)
            self.assertEqual(len(set(m["commits"])), 100)
            self.assertEqual(len({a["review_cwd"] for a in m["assignments"]}), 100)
            self.assertTrue(
                all(not c["passed"] for c in json.loads((Path(tmp) / "baseline.json").read_text()))
            )
            corpus.preflight(m)
            p = Path(m["assignments"][0]["working_file"])
            atomic_write_text(p, p.read_text() + "\n")
            with self.assertRaisesRegex(RuntimeError, "Prepared working tree is not clean"):
                corpus.preflight(m)


class MemoryTests(unittest.TestCase):
    def test_sampler_tracks_orphans_but_not_reused_pids(self):
        def row(pid, parent, born=1):
            return {
                "pid": pid,
                "ppid": parent,
                "pgid": pid,
                "rss_kib": 10,
                "start_time": born,
                "executable": "fake",
            }

        with (
            tempfile.TemporaryDirectory() as tmp,
            patch("e2e.scripts.scenario_memory.host_snapshot", return_value={}),
        ):
            sampler = Sampler(1, tmp)
            table = {1: row(1, 0), 2: row(2, 1), 3: row(3, 2)}
            with patch("e2e.scripts.scenario_memory.process_table", return_value=table):
                self.assertEqual(sampler.sample()["provider_tree_rss_kib"], 20)
            del table[2]
            table[3]["ppid"] = 99
            with patch("e2e.scripts.scenario_memory.process_table", return_value=table):
                self.assertEqual(sampler.sample()["provider_tree_rss_kib"], 10)
            table[3]["start_time"] = 5
            with patch("e2e.scripts.scenario_memory.process_table", return_value=table):
                self.assertEqual(sampler.sample()["provider_tree_rss_kib"], 0)

    def test_aggregate_peak_not_sum_of_individual_peaks(self):
        agents = [{"id": str(i), "role": "reviewer"} for i in (2, 3)]
        events = {
            a["id"]: [
                {
                    "type": "provider_started",
                    "timestamp": 20,
                    "data": {"pid": int(a["id"]), "spawn_before": 1, "spawn_after": 2},
                },
                {"type": "session_ready", "timestamp": 20, "data": {}},
                {"type": "provider_shutdown", "timestamp": 40, "data": {}},
            ]
            for a in agents
        }
        rows = []
        for t, rss in ((21, (100, 10)), (22, (10, 100))):
            processes = [
                {
                    "pid": pid,
                    "ppid": 1,
                    "launcher_pid": pid,
                    "launcher_start_time": 1,
                    "start_time": 1,
                    "rss_kib": value,
                }
                for pid, value in zip((2, 3), rss)
            ]
            rows.append(
                {"time": t, "daemon": {"pid": 1}, "daemon_rss_kib": 5, "processes": processes}
            )
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "memory.jsonl"
            atomic_write_text(path, "".join(json.dumps(r) + "\n" for r in rows))
            result = summarize(path, agents, events)
            self.assertEqual(result["peak"]["total_rss_kib"], 115)
            self.assertEqual(result["all_agents_loaded_samples"], 2)
            self.assertEqual(result["observed_interval_seconds"]["max"], 1)
            self.assertEqual(result["sampling_gaps_over_5_seconds"], [])
            gap_row = copy.deepcopy(rows[-1])
            gap_row["time"] = 866
            atomic_write_text(path, "".join(json.dumps(r) + "\n" for r in [*rows, gap_row]))
            result = summarize(path, agents, events)
            self.assertEqual(result["observed_interval_seconds"]["max"], 844)
            self.assertEqual(
                result["sampling_gaps_over_5_seconds"], [{"start": 22, "end": 866, "seconds": 844}]
            )
            rows[0]["processes"][0]["launcher_start_time"] = 100
            rows[1]["processes"][0]["launcher_start_time"] = 100
            atomic_write_text(path, "".join(json.dumps(r) + "\n" for r in rows))
            with self.assertRaisesRegex(
                RuntimeError, "No sample observed all 101 loaded sessions alive together"
            ):
                summarize(path, agents, events)


if __name__ == "__main__":
    unittest.main()
