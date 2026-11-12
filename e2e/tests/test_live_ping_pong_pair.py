"""Negative evidence checks, native footprint smoke, and no-inference preparation."""

import copy
import ctypes
import os
import platform
import sqlite3
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from release.files import atomic_write_bytes, create_directory
from e2e.scripts.live_pair_memory import FootprintSampler, Usage, shared_owners
from e2e.scripts.run_live_ping_pong_pair import PHASES, Run
from release.progress import entry, phase
from e2e.scripts.scenario_memory import process_table
from e2e.scripts.verify_live_ping_pong_pair import verify


def fixture():
    agents, events, messages = [], {}, []

    def ev(aid, kind, data=None):
        seq = sum(map(len, events.values())) + 1
        item = dict(seq=seq, type=kind, data=data or {}, timestamp=seq, execution_id=aid + "-exec")
        events[aid].append(item)
        return item

    def msg(mid, sender, recipient, body, state="completed"):
        messages.append(dict(id=mid, sender=sender, recipient=recipient, body=body, state=state))

    def turn(aid, mid):
        return ev(aid, "turn_started", dict(message_id=mid, turn_id=mid + "-turn"))

    for aid, role, parent, outcome in (
        ("p", "live-ping", None, "completed"),
        ("c", "live-pong", "p", "cancelled"),
    ):
        events[aid] = []
        agents.append(
            dict(
                id=aid,
                role=role,
                parent_id=parent,
                task_outcome=outcome,
                state="stopped",
                turn_id=None,
                control=None,
                error=None,
                expired=False,
                lifetime="independent",
                model=role,
                effort="medium",
                thread_id=aid + "-thread",
            )
        )
    for aid in ("p", "c"):
        ev(aid, "execution_starting")
        ev(
            aid,
            "session_ready",
            dict(
                model=agents[0 if aid == "p" else 1]["model"],
                effort="medium",
                thread_id=aid + "-thread",
            ),
        )
    msg("i", "human", "p", "INIT n PREFLIGHT")
    ev("p", "spawned", dict(initial_message_id="i", parent_id=None))
    turn("p", "i")
    msg("s", "p", "c", "INIT n PREFLIGHT")
    ev("c", "spawned", dict(initial_message_id="s", parent_id="p"))
    msg("go", "human", "p", "START n")
    turn("p", "go")
    msg("ping", "p", "c", "PING n")
    ev("c", "message_accepted", dict(id="ping", sender_turn_id="go-turn"))
    turn("c", "ping")
    msg("pong", "c", "p", "PONG n")
    ev("p", "message_accepted", dict(id="pong", sender_turn_id="ping-turn"))
    ev("c", "turn_finished", dict(message_id="ping", status="completed"))
    turn("p", "pong")
    ev("c", "termination_requested", dict(by="p"))
    ev("p", "completion_requested", dict(by="p"))
    for aid in ("p", "c"):
        ev(aid, "provider_shutdown", dict(confirmed=True, quiescent=True))
        ev(aid, "execution_stopped")
    requests = [
        dict(sender=sender, id=prefix + "n", body=dict(op=op, target=target))
        for sender, prefix, op, target in (
            ("p", "ping-", "send", "@child"),
            ("c", "pong-", "send", "@parent"),
            ("p", "stop-", "terminate", "@child"),
        )
    ]
    return dict(
        agents=agents,
        events=events,
        messages=messages,
        requests=requests,
        roots={"p": "n"},
        initial_bodies={"n": "INIT n PREFLIGHT"},
        expected_roles={r: dict(model=r, effort="medium") for r in ("live-ping", "live-pong")},
    )


class VerificationTests(unittest.TestCase):
    def test_readiness_rejects_a_message_or_ended_task_before_start(self):
        with sqlite3.connect(":memory:") as database:
            database.execute("CREATE TABLE messages(recipient TEXT, startup INTEGER, state TEXT)")
            run = Run.__new__(Run)
            run.database = database
            agent = dict(id="p", state="idle", task_outcome="open", turn_id=None, thread_id="t")
            run.observe = lambda: [agent]
            database.execute("INSERT INTO messages VALUES ('p', 0, 'completed')")
            with self.assertRaisesRegex(RuntimeError, "message arrived before START"):
                run.wait_ready(1)
            database.execute("DELETE FROM messages")
            agent["task_outcome"] = "completed"
            with self.assertRaisesRegex(RuntimeError, "agent ended before START"):
                run.wait_ready(1)

    def test_active_off_workspace_child_prevents_a_false_dead_end(self):
        with sqlite3.connect(":memory:") as database:
            database.execute(
                "CREATE TABLE agents(id,state,turn_id,control,completion_pending,task_outcome,expired)"
            )
            database.execute("CREATE TABLE messages(recipient,state)")
            database.executemany(
                "INSERT INTO agents VALUES(?,?,NULL,NULL,NULL,'open',0)",
                [("p", "idle"), ("off-workspace-child", "running")],
            )
            run = Run.__new__(Run)
            run.database = database
            owned = [dict(id="p"), dict(id="off-workspace-child")]
            run.reject_dead_end(owned)
            database.execute("UPDATE agents SET state='idle'")
            with self.assertRaisesRegex(RuntimeError, "no active turn"):
                run.reject_dead_end(owned)

    def test_unsettled_spawn_blocks_cleaned_after_other_teardown_finishes(self):
        run = Run.__new__(Run)
        run.timings, run.roots = {}, {}
        run.pending_spawns = {"preflight-spawn"}
        run.database = run.sampler = None
        run.daemon, run.success = {"pid": 123}, False
        closed, phases = [], []
        run.daemon_owner = SimpleNamespace(close=lambda: closed.append(True))
        run.admission = SimpleNamespace(advance=lambda phase: phases.append(phase))
        run.discover = lambda: []
        run.save = lambda *args: None
        run.call = None
        with patch("e2e.scripts.run_live_ping_pong_pair.verify_tasks"):
            with self.assertRaisesRegex(RuntimeError, "Spawn acceptance remains unresolved"):
                run.cleanup()
        self.assertEqual(closed, [True])
        self.assertFalse(phases)

    def test_preflight_tool_checks_each_provider_native_result_shape(self):
        for provider in ("codex", "claude"):
            with self.subTest(provider=provider):
                run = Run.__new__(Run)
                run.providers = {"live-pong": provider}
                run.discover = lambda: [dict(id="child", role="live-pong")]
                run.call = None
                run.save = lambda *args: None
                content = [
                    dict(type="inputText" if provider == "codex" else "text", text='{"agents":[]}')
                ]
                item = (
                    dict(type="dynamicToolCall", success=True, contentItems=content)
                    if provider == "codex"
                    else dict(result=dict(content=content, is_error=False))
                )
                with patch(
                    "e2e.scripts.run_live_ping_pong_pair.event_pages",
                    return_value=[dict(type="tool_completed", data=dict(item=item))],
                ):
                    run.preflight_tools()

    def test_dead_end_preserves_pending_and_completion_work(self):
        with sqlite3.connect(":memory:") as db:
            db.execute(
                "CREATE TABLE agents(id,data,state,turn_id,control,completion_pending,"
                "task_outcome DEFAULT 'open',expired DEFAULT 0)"
            )
            db.execute("CREATE TABLE messages(recipient,state)")
            db.execute("INSERT INTO agents(id,data,state) VALUES('p','{\"cwd\":\"/work\"}','idle')")
            run = Run.__new__(Run)
            run.database, run.work = db, Path("/work")
            with self.assertRaisesRegex(RuntimeError, "no active turn"):
                run.reject_dead_end([dict(id="p")])
            db.execute("INSERT INTO messages VALUES('p','accepted')")
            run.reject_dead_end([dict(id="p")])
            db.execute("UPDATE messages SET state='completed'")
            db.execute("UPDATE agents SET completion_pending='{}'")
            run.reject_dead_end([dict(id="p")])
            db.execute("UPDATE agents SET completion_pending=NULL,state='stopped'")
            run.reject_dead_end([dict(id="p")])  # Last completion raced the earlier CLI snapshot.
            for outcome, expired in [("completed", 0), ("cancelled", 0), ("open", 1)]:
                with self.subTest(outcome=outcome, expired=expired):
                    db.execute(
                        "UPDATE agents SET state='idle',task_outcome=?,expired=?",
                        (outcome, expired),
                    )
                    run.reject_dead_end(
                        [dict(id="p")]
                    )  # Terminal, but provider shutdown is still pending.

    def test_progress_accepts_all_live_phases(self):
        def exercise():
            for name in ["daemon", "verify", *PHASES.values()]:
                with phase(name):
                    self.assertTrue(True)

        self.assertEqual(entry(exercise, name="scenario"), 0)

    def test_valid(self):
        self.assertEqual(verify(fixture())["confirmed_conversation_releases"], 2)

    def test_shared_release_is_accepted_but_late_release_is_not(self):
        value = fixture()
        release = value["events"]["p"][-2]
        release["type"] = "provider_session_released"
        self.assertEqual(verify(value)["relative_target_requests"], 3)
        release["seq"] = value["events"]["p"][-1]["seq"] + 1
        with self.assertRaisesRegex(RuntimeError, "Unconfirmed release"):
            verify(value)

    def test_cleanup_stops_daemon_when_sampler_construction_failed(self):
        run = Run.__new__(Run)
        run.timings, run.roots = {}, {}
        run.admission = None
        run.pending_spawns = set()
        run.database = run.sampler = None
        run.daemon, run.success = {"pid": 123}, False
        calls, saved = [], {}
        run.call = lambda *args: calls.append(args)
        run.daemon_owner = SimpleNamespace(close=lambda: run.call("daemon", "stop"))
        run.discover = lambda: []
        run.save = lambda name, value: saved.update({name: value})
        with patch("e2e.scripts.run_live_ping_pong_pair.verify_tasks"):
            run.cleanup()
        self.assertEqual(calls, [("daemon", "stop")])
        self.assertIn("cleanup_finished", saved["timings"])

    def test_shared_provider_grouping(self):
        value = dict(
            agents=[dict(id="a", role="ping"), dict(id="b", role="ping")],
            events={
                aid: [
                    dict(
                        type="provider_started",
                        data=dict(provider_id="shared", pid=123, spawn_before=10, spawn_after=11),
                    )
                ]
                for aid in ("a", "b")
            },
        )
        owners = shared_owners(value)
        self.assertEqual(len(owners), 1)
        self.assertEqual(owners[123]["agents"], ["a", "b"])
        value["agents"][1]["role"] = "pong"
        self.assertEqual(sorted(shared_owners(value)[123]["roles"]), ["ping", "pong"])
        value["events"]["b"][0]["data"]["provider_id"] = "other"
        with self.assertRaisesRegex(RuntimeError, "PID reuse"):
            shared_owners(value)

    def test_wrong_causality(self):
        value = fixture()
        next(e for e in value["events"]["p"] if e["type"] == "message_accepted")["data"][
            "sender_turn_id"
        ] = "other"
        with self.assertRaisesRegex(RuntimeError, "Wrong sending turn"):
            verify(value)

    def test_bad_evidence(self):
        baseline = fixture()
        mutations = [
            lambda e: e["messages"].append(copy.deepcopy(e["messages"][0])),
            lambda e: e["agents"][1].update(thread_id="p-thread"),
            lambda e: e["agents"][1].update(effort="high"),
            lambda e: e["agents"][1].update(parent_id=None),
            lambda e: e["events"]["c"][-2]["data"].update(confirmed=False),
            lambda e: e["requests"][1]["body"].update(target="p"),
            lambda e: e["events"]["c"][-2]["data"].update(quiescent=False),
            lambda e: next(x for x in e["events"]["c"] if x["type"] == "termination_requested")[
                "data"
            ].update(by="human"),
            lambda e: e["messages"][3].update(state="uncertain"),
        ]
        for change in mutations:
            with self.subTest(change=mutations.index(change)):
                value = copy.deepcopy(baseline)
                change(value)
                with self.assertRaises(RuntimeError):
                    verify(value)


@unittest.skipUnless(platform.system() == "Darwin", "macOS accounting")
class MemoryTests(unittest.TestCase):
    def test_native_footprint_and_pid_reuse(self):
        with tempfile.TemporaryDirectory() as directory:
            sampler = FootprintSampler(os.getpid(), directory)
            row = process_table()[os.getpid()]
            result = sampler.measure(row)
            self.assertTrue(result["complete"])
            self.assertGreater(result["bytes"], 0)
            self.assertEqual(ctypes.sizeof(Usage), 96)
            sampler.kernel_starts[(row["pid"], row["start_time"])] += 1
            self.assertFalse(sampler.measure(row)["complete"])
            self.assertFalse(sampler.measure(dict(pid=2147483647, start_time=0))["complete"])

    def test_cleanup_waits_for_acknowledged_daemon_and_orphan_exit(self):
        with tempfile.TemporaryDirectory() as directory:
            sampler = FootprintSampler(123, directory)
            sampler.daemon_start = 10
            sampler.owned[(456, 11)] = (456, 11)
            daemon, child = dict(start_time=10), dict(start_time=11)
            with (
                patch(
                    "e2e.scripts.live_pair_memory.process_table",
                    side_effect=[{123: daemon, 456: child}, {456: child}, {}],
                ),
                patch("e2e.scripts.live_pair_memory.pause") as sleep,
            ):
                sampler.verify_gone()
            self.assertEqual(sleep.call_count, 2)
            self.assertTrue((Path(directory) / "process-cleanup.json").exists())

    def test_expected_daemon_exit_keeps_orphans_in_measurement(self):
        with tempfile.TemporaryDirectory() as directory:
            sampler = FootprintSampler(123, directory)
            sampler.daemon_start, sampler.expect_shutdown = 10, True
            sampler.owned[(456, 11)] = (456, 11)
            row = dict(pid=456, ppid=1, start_time=11, rss_kib=100)
            with (
                patch("e2e.scripts.scenario_memory.process_table", return_value={456: row}),
                patch.object(sampler, "measure", return_value=dict(complete=True, bytes=50)),
            ):
                sample = sampler.sample()
            self.assertTrue(sample["daemon"]["exited"])
            self.assertEqual(sample["total_rss_kib"], 100)
            with patch("e2e.scripts.live_pair_memory.process_table", return_value={456: row}):
                with self.assertRaisesRegex(RuntimeError, "processes remain"):
                    sampler.verify_gone(timeout=0)
            with patch("e2e.scripts.live_pair_memory.process_table", return_value={}):
                sampler.verify_gone()
            sampler.expect_shutdown = False
            with patch("e2e.scripts.scenario_memory.process_table", return_value={}):
                with self.assertRaisesRegex(RuntimeError, "Daemon disappeared"):
                    sampler.sample()

    def test_partial_sample_is_not_zero(self):
        with tempfile.TemporaryDirectory() as directory:
            sampler = FootprintSampler(os.getpid(), directory)
            with patch.object(sampler, "measure", return_value=dict(complete=False, errno=3)):
                sample = sampler.sample()
            self.assertFalse(sample["footprint_complete"])
            self.assertIsNone(sample["total_footprint_bytes"])

    def test_preparation_has_no_inference(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            create_directory(root / "release")
            atomic_write_bytes(root / "release/hivectl", b"fixture, not executable")
            platform.platform()  # Cache the harmless uname probe before denying commands.
            with patch("subprocess.run", side_effect=AssertionError("Unexpected command")):
                run = Run(
                    SimpleNamespace(
                        binary=root / "release/hivectl",
                        output_dir=root / "output",
                        state_dir=root / "state",
                        pairs=50,
                        timeout=1800,
                        max_starting=1,
                        providers="codex",
                        codex_model="gpt-6.1-terra",
                        claude_model="claude-sonnet-5-5",
                    )
                )
            self.assertEqual(run.roots, {})
            self.assertTrue(run.config.is_file())
            import json

            self.assertEqual(json.loads(run.config.read_text())["runtime"]["max_starting"], 1)
            self.assertNotIn(
                "sessions_per_codex_process", json.loads(run.config.read_text())["runtime"]
            )
            self.assertEqual(json.loads(run.config.read_text())["runtime"]["idle_seconds"], 1800)
            for role in ("live-ping", "live-pong"):
                selected = json.loads(run.config.read_text())["roles"][role]
                self.assertEqual((selected["model"], selected["effort"]), ("gpt-6.1-terra", "low"))
