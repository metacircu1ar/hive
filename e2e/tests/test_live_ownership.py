"""Offline live-scenario admission and real fixture-service ownership."""

from contextlib import contextmanager
import contextlib
import copy
import fcntl
import io
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from e2e.live import ownership as live
from e2e.scripts import run_live_ping_pong_pair as pairs
from e2e.scripts import run_ping_pong_all_to_one as joined
from e2e.scripts.live_pair_memory import summarize
from release.cancellation import Cancelled
from release.files import atomic_write_bytes, create_directory
from release.process import ProcessFailure
from release.progress import entry, finalizer
from release.temporary import directory

FIXTURE = Path(__file__).parent / "fixtures/live_daemon.py"


@contextmanager
def state_fixture():
    with directory(prefix="hive-live-", parent=Path("/tmp")) as state:
        create_directory(state / "runtime", mode=0o700)
        atomic_write_bytes(state / "runtime/daemon.lock", b"")
        with sqlite3.connect(state / "runtime/runtime.sqlite") as database:
            database.executescript("""
              CREATE TABLE agents(id,task_outcome,expired,state,data,thread_id,listing_seq,execution_id,provider_id);
              CREATE TABLE comb_collectors(name,enabled,paused,pending,gap_count,last_error);
              CREATE TABLE comb_outbox(sequence,destination,comb_id,body);
              CREATE TABLE provider_events(seq INTEGER PRIMARY KEY,provider,at,kind,data);
            """)
        yield state


class AdmissionTests(unittest.TestCase):
    def test_daemon_and_auth_probes_use_the_owned_config_not_registered_service_scope(self):
        call = live.configured_cli(
            ["fixture/release/hivectl", "--state-dir", "/state"], Path("/run/config.json")
        )
        for family in ("daemon", "auth", "models"):
            self.assertEqual(
                call.argv(family, "status"),
                [
                    "fixture/release/hivectl",
                    "--state-dir",
                    "/state",
                    "--config",
                    "/run/config.json",
                    family,
                    "status",
                ],
            )

    def test_cleanup_follows_changed_workspaces_and_retries_after_partial_failure(self):
        agents = [
            dict(
                id="root",
                parent_id=None,
                cwd="/owned",
                task_outcome="open",
                expired=False,
                state="idle",
            ),
            dict(
                id="child",
                parent_id="root",
                cwd="/elsewhere",
                task_outcome="open",
                expired=False,
                state="idle",
            ),
        ]
        attempts = []
        failed = [False]

        def call(*args):
            if args[0] == "agents":
                return dict(agents=copy.deepcopy(agents), next_after=None)
            agent = next(a for a in agents if a["id"] == args[1])
            if args[0] == "status":
                return dict(agent=copy.deepcopy(agent))
            if args[0] == "stop":
                agent["state"] = "stopped"
                return {}
            if args[0] != "terminate":
                raise AssertionError("Unexpected cleanup command")
            attempts.append(agent["id"])
            if agent["id"] == "root" and not failed[0]:
                failed[0] = True
                raise OSError("Fixture transient termination failure")
            if agent["id"] == "child":
                agents.append(
                    dict(
                        id="late",
                        parent_id="child",
                        cwd="/third-place",
                        task_outcome="open",
                        expired=False,
                        state="idle",
                    )
                )
            agent.update(task_outcome="cancelled", state="stopped")
            return {}

        discover = lambda: live.owned_agents(call, Path("/owned"), ["root"])
        self.assertEqual(live.cancel_descendants(call, "root", discover), [])
        self.assertEqual(attempts, ["root", "child", "root", "late"])

    def test_empty_xdg_state_home_matches_the_product_default(self):
        with patch.dict(os.environ, {"XDG_STATE_HOME": ""}):
            self.assertEqual(live.default_state(), Path.home() / ".local/state/hive")

    def test_disabled_unpaused_collector_refuses_before_daemon_or_other_work(self):
        for pending, gap in ((1, 0), (0, 1)):
            with self.subTest(pending=pending, gap=gap), state_fixture() as state:
                with sqlite3.connect(state / "runtime/runtime.sqlite") as database:
                    database.execute(
                        "INSERT INTO comb_collectors VALUES('other',0,0,?,?,'held')", (pending, gap)
                    )
                owner = live.Daemon(None, state, state / "unused", state)
                with (
                    patch.object(live, "spawn") as spawn,
                    contextlib.redirect_stdout(io.StringIO()) as output,
                ):
                    with self.assertRaisesRegex(RuntimeError, "Unpaused"):
                        owner.start()
                    spawn.assert_not_called()
                self.assertTrue(json.loads(output.getvalue())["pre_launch"])
                with sqlite3.connect(state / "runtime/runtime.sqlite") as database:
                    self.assertEqual(
                        database.execute("SELECT * FROM comb_collectors").fetchone(),
                        ("other", 0, 0, pending, gap, "held"),
                    )

    def test_reserved_collector_refuses_even_paused_backlog_before_launch(self):
        with state_fixture() as state:
            with sqlite3.connect(state / "runtime/runtime.sqlite") as database:
                database.execute("INSERT INTO comb_collectors VALUES('hive-e2e',0,1,1,0,'held')")
            owner = live.Daemon(None, state, state / "unused", state, collectors=("hive-e2e",))
            with (
                patch.object(live, "spawn") as spawn,
                contextlib.redirect_stdout(io.StringIO()) as output,
            ):
                with self.assertRaises(RuntimeError):
                    owner.start()
                spawn.assert_not_called()
            recovery = json.loads(output.getvalue())["reserved_recovery"]["hive-e2e"]
            self.assertEqual(
                recovery["inspect"], ["hivectl", "--state-dir", str(state), "collectors", "status"]
            )
            self.assertIn("hive-e2e", recovery["discard_after_inspection"])

    def test_only_empty_reserved_collector_can_change_binding(self):
        with state_fixture() as state:
            with sqlite3.connect(state / "runtime/runtime.sqlite") as database:
                database.execute("INSERT INTO comb_collectors VALUES('hive-e2e',0,1,0,0,NULL)")
                database.execute("INSERT INTO comb_collectors VALUES('other',0,1,1,0,'held')")
            with contextlib.redirect_stdout(io.StringIO()):
                before = live.preflight(state, collectors=("hive-e2e",))
            self.assertEqual(set(before.paused), {"other"})

    def test_paused_other_evidence_is_allowed_and_checked_after_config_disables_it(self):
        with state_fixture() as state:
            with sqlite3.connect(state / "runtime/runtime.sqlite") as database:
                database.execute("INSERT INTO comb_collectors VALUES('other',1,1,1,0,'held')")
                database.execute("INSERT INTO comb_outbox VALUES(1,'other','bound','kept')")
            with contextlib.redirect_stdout(io.StringIO()):
                before = live.preflight(state)
            with sqlite3.connect(state / "runtime/runtime.sqlite") as database:
                database.execute("UPDATE comb_collectors SET enabled=0")
            live.verify_paused(state, before.paused)
            with sqlite3.connect(state / "runtime/runtime.sqlite") as database:
                database.execute("UPDATE comb_outbox SET body='changed'")
            with self.assertRaisesRegex(RuntimeError, "changed"):
                live.verify_paused(state, before.paused)

    def test_stopped_open_task_is_not_accepted_as_clean_history(self):
        with state_fixture() as state:
            with sqlite3.connect(state / "runtime/runtime.sqlite") as database:
                database.execute(
                    "INSERT INTO agents(id,task_outcome,expired,state) VALUES('a','open',0,'stopped')"
                )
            with self.assertRaisesRegex(RuntimeError, "unfinished"):
                live.preflight(state)
            with sqlite3.connect(state / "runtime/runtime.sqlite") as database:
                database.execute("UPDATE agents SET expired=1")
            self.assertEqual(live.preflight(state).paused, {})

    def test_product_lock_is_exact_and_new_missing_lock_is_not_created(self):
        with state_fixture() as state:
            path = state / "runtime/daemon.lock"
            with path.open("rb") as held:
                fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
                with self.assertRaisesRegex(RuntimeError, "existing daemon"):
                    live.preflight(state)
            live.preflight(state)
        with tempfile.TemporaryDirectory() as temporary:
            state = Path(temporary).resolve()
            self.assertEqual(live.preflight(state).paused, {})
            self.assertFalse((state / "runtime").exists())

    def test_no_opt_in_never_probes_a_binary_or_login(self):
        for module in (pairs, joined):
            with (
                self.subTest(module=module.__name__),
                patch("sys.argv", ["scenario"]),
                patch.object(module, "require_paths") as binary,
                contextlib.redirect_stderr(io.StringIO()),
            ):
                with self.assertRaises(SystemExit) as refused:
                    module.parse_args()
                self.assertEqual(refused.exception.code, 2)
                binary.assert_not_called()

    def test_defaults_and_original_scale_are_explicit(self):
        with (
            patch.object(pairs, "require_paths"),
            patch("sys.argv", ["pairs", "--live"]),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            args = pairs.parse_args()
        self.assertEqual((args.pairs, args.providers, args.timeout), (1, "codex", 600))
        self.assertEqual(args.codex_model, "gpt-6.1-terra")
        with (
            patch.object(pairs, "require_paths"),
            patch("sys.argv", ["pairs", "--live", "--pairs", "50", "--providers", "mixed"]),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            args = pairs.parse_args()
        self.assertEqual((args.pairs, args.providers, args.timeout), (50, "mixed", 1800))
        for count in (2, 10):
            with (
                patch.object(joined, "require_paths"),
                patch("sys.argv", ["all-to-one", "--live", "--children", str(count)]),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                args, _ = joined.parse_args()
            self.assertEqual((args.children, args.expected_model, args.expected_effort),
                             (count, "gpt-6.1-terra", "low"))

    def test_lost_spawn_response_keeps_exact_request_and_refuses_duplicate(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)

            class LostReply:
                def __init__(self):
                    self.calls = []

                def argv(self, *args):
                    return ["fixture/release/hivectl", *args]

                def __call__(self, *args):
                    self.calls.append(args)
                    raise TimeoutError("Reply lost after acceptance")

            call = LostReply()
            with self.assertRaises(TimeoutError):
                joined.spawn_root(call, root, root, "task")
            intent = json.loads((root / "spawn-intent.json").read_text())
            self.assertEqual(intent["command"], call.argv(*call.calls[0]))
            self.assertEqual(intent["request_id"], call.calls[0][-1])
            with self.assertRaisesRegex(RuntimeError, "Unresolved spawn intent"):
                joined.spawn_root(call, root, root, "task")
            self.assertEqual(len(call.calls), 1)

    def test_cleanup_rediscovery_ends_independent_grandchild_and_pauses_failed_termination(self):
        agents = [
            dict(id="root", parent_id=None, task_outcome="open", expired=False, state="idle"),
            dict(id="child", parent_id="root", task_outcome="open", expired=False, state="idle"),
        ]
        calls = []

        def call(*argv):
            calls.append(argv)
            if argv[0] == "terminate":
                target = next(a for a in agents if a["id"] == argv[1])
                if target["id"] == "child":
                    agents.append(
                        dict(
                            id="late",
                            parent_id="child",
                            task_outcome="open",
                            expired=False,
                            state="idle",
                        )
                    )
                target.update(task_outcome="cancelled", state="stopped")

        self.assertEqual(live.cancel_descendants(call, "root", lambda: copy.deepcopy(agents)), [])
        self.assertEqual([args[1] for args in calls], ["root", "child", "late"])
        failed = dict(id="failed", parent_id=None, task_outcome="open", expired=False, state="idle")

        def refuse(*argv):
            calls.append(argv)
            if argv[0] == "terminate":
                raise ProcessFailure(subprocess.CompletedProcess([], 1, "", ""))
            if argv[0] == "status":
                return dict(agent=failed)

        with self.assertRaises(TimeoutError):
            live.cancel_descendants(refuse, "failed", lambda: [failed], timeout=0.01)
        self.assertEqual(calls[-1], ("stop", "failed"))


class MemoryTests(unittest.TestCase):
    def test_shared_cross_role_process_is_counted_once_on_linux(self):
        evidence = dict(
            agents=[dict(id="a", role="ping"), dict(id="b", role="pong")],
            events={
                aid: [
                    dict(
                        type="provider_started",
                        data=dict(provider_id="shared", pid=10, spawn_before=1, spawn_after=3),
                    )
                ]
                for aid in ("a", "b")
            },
        )
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "memory.jsonl"
            rows = [
                dict(
                    monotonic_time=i,
                    processes=[
                        dict(
                            pid=10,
                            launcher_pid=10,
                            start_time=2,
                            launcher_start_time=2,
                            rss_kib=1024,
                        )
                    ],
                    total_rss_kib=2048,
                    footprint_available=False,
                    footprint_complete=False,
                    total_footprint_bytes=None,
                )
                for i in range(4)
            ]
            atomic_write_bytes(path, b"".join((json.dumps(row) + "\n").encode() for row in rows))
            result = summarize(path, evidence, [0, 3])
            self.assertEqual(result["provider_processes"], 1)
            self.assertEqual(result["process_trees"]["10"]["roles"], ["ping", "pong"])
            self.assertEqual(result["process_trees"]["10"]["rss_mib"]["max"], 1)
            self.assertIsNone(result["peak_complete_total_footprint_gib"])


class ProcessTests(unittest.TestCase):
    def test_known_preflight_refusal_and_proved_cleanup_clear_only_our_admission(self):
        from e2e.campaign import read_receipt

        for phase in ("prepared", "cleaned"):
            with self.subTest(phase=phase), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                args = SimpleNamespace(state_dir=root / "state", output_dir=root / "out")

                def work():
                    with live.campaign(args, "fixture") as admission:
                        admission.advance(phase)
                        raise RuntimeError("Fixture refusal after known cleanup")

                with (
                    patch("e2e.campaign.default_root", return_value=root / "admission"),
                    contextlib.redirect_stdout(io.StringIO()),
                ):
                    self.assertEqual(entry(work, name="scenario"), 1)
                self.assertIsNone(read_receipt(root / "admission"))

    def test_actual_service_is_retired_before_cancellation_returns(self):
        for foreign in (False, True):
            with self.subTest(foreign=foreign), state_fixture() as root:
                calls = []

                class Call:
                    command = [sys.executable, str(FIXTURE), str(root)]

                    def argv(self, *args):
                        return list(args)

                    def __call__(self, *args):
                        calls.append(args)
                        if args != ("daemon", "stop"):
                            raise AssertionError("Unexpected fixture command")
                        atomic_write_bytes(root / "stop", b"stop")

                def status(*args, **options):
                    ready = root / "ready.json"
                    value = (
                        dict(pid=1 if foreign else json.loads(ready.read_text())["pid"])
                        if ready.exists()
                        else dict(error=dict(code="daemon_unavailable"))
                    )
                    return subprocess.CompletedProcess(
                        args, 0 if "pid" in value else 1, json.dumps(value), ""
                    )

                owner = live.Daemon(Call(), root, root / "unused", root)

                def work():
                    with finalizer(owner.close):
                        owner.start()
                        raise Cancelled(signal.SIGTERM)

                with patch.object(live, "run", side_effect=status):
                    self.assertEqual(entry(work, name="scenario"), 1 if foreign else 143)
                self.assertTrue(owner.process.completed)
                with self.assertRaises(ProcessLookupError):
                    os.killpg(owner.process.pid, 0)
                self.assertEqual(calls, [])

    def test_failure_retains_admission_until_manual_cleanup_confirmation(self):
        from e2e.campaign import read_receipt

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = SimpleNamespace(state_dir=root / "state", output_dir=root / "out")

            def work():
                with live.campaign(args, "fixture") as admission:
                    admission.advance("starting")
                    raise Cancelled(signal.SIGTERM)

            with (
                patch("e2e.campaign.default_root", return_value=root / "admission"),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                self.assertEqual(entry(work, name="scenario"), 143)
            self.assertEqual(read_receipt(root / "admission")["kind"], "native")


if __name__ == "__main__":
    unittest.main()
