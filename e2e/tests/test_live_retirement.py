"""A stopped task or reaped daemon is not by itself a provider retirement proof."""

import copy
import json
import sqlite3
import subprocess
import sys
import time
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from e2e.live import ownership as live
from e2e.live.proof import RetirementFailure, Snapshot, verify
from e2e.scripts.harness import cli
from e2e.tests.test_live_ownership import FIXTURE, state_fixture
from e2e.tests.fixtures.campaign_service import request
from release.cancellation import finish, pause
from release.files import atomic_write_bytes
from release.progress import entry, finalizer


class RetirementTests(unittest.TestCase):
    def test_fresh_failed_bootstrap_needs_the_product_clean_proof(self):
        with (
            state_fixture() as state,
            sqlite3.connect(state / "runtime/runtime.sqlite") as database,
        ):
            database.execute(
                "INSERT INTO agents(id,data,state,task_outcome,expired,thread_id,listing_seq,execution_id) VALUES('fresh',?,'stopped','cancelled',0,NULL,1,'reserved')",
                (json.dumps(dict(conversation_clean=False)),),
            )
            with self.assertRaises(RetirementFailure) as caught:
                verify(database, Snapshot({}, 0, 0))
            self.assertEqual(
                caught.exception.report,
                dict(kind="agent", identity="fresh", reason="conversation_unconfirmed"),
            )
            database.execute(
                "UPDATE agents SET data=? WHERE id='fresh'",
                (json.dumps(dict(conversation_clean=True)),),
            )
            verify(database, Snapshot({}, 0, 0))

    def test_preassigned_claude_session_is_not_evidence_of_native_launch(self):
        for variant in ("omitted", "false", "attempted", "execution", "provider"):
            with (
                self.subTest(variant=variant),
                state_fixture() as state,
                sqlite3.connect(state / "runtime/runtime.sqlite") as database,
            ):
                data = dict(conversation_clean=False, profile=dict(provider="claude"))
                if variant in ("false", "attempted"):
                    data["native_input_attempted"] = variant == "attempted"
                database.execute(
                    "INSERT INTO agents(id,data,state,task_outcome,expired,thread_id,listing_seq,execution_id,provider_id) VALUES('queued',?,'stopped','cancelled',0,'preassigned',1,?,?)",
                    (
                        json.dumps(data),
                        "e" if variant == "execution" else None,
                        "p" if variant == "provider" else None,
                    ),
                )
                if variant in ("omitted", "false"):
                    verify(database, Snapshot({}, 0, 0))
                else:
                    with self.assertRaises(RuntimeError):
                        verify(database, Snapshot({}, 0, 0))

    def test_remote_terminal_state_cannot_erase_uncertain_native_cli_cleanup(self):
        from e2e.campaign import read_receipt

        for wrapped in (False, True):
            for primary in (False, True):
                with self.subTest(wrapped=wrapped, primary=primary), state_fixture() as root:
                    agent = dict(
                        id="root", parent_id=None, task_outcome="open", expired=False, state="idle"
                    )
                    native = OSError("Fixture local CLI cleanup is uncertain")
                    native.owned_cleanup_uncertain = True
                    failure = (
                        OSError("Fixture publication wraps the uncertain owner")
                        if wrapped
                        else native
                    )
                    if wrapped:
                        failure.__cause__ = native

                    def call(*args):
                        if args[0] == "status":
                            return dict(agent=dict(agent))
                        if args[0] == "terminate":
                            agent.update(task_outcome="cancelled", state="stopped")
                            raise failure
                        raise AssertionError("Unexpected local CLI call")

                    accepted, services = [], []

                    def cleanup():
                        with finalizer(lambda: services.append("closed")):
                            live.cancel_descendants(call, "root", lambda: [dict(agent)])

                    def work():
                        args = SimpleNamespace(state_dir=root, output_dir=root)
                        with live.campaign(args, "fixture") as admission:
                            admission.advance("starting")
                            with finalizer(cleanup):
                                if primary:
                                    raise RuntimeError("Fixture work failure stays primary")
                            admission.advance("cleaned")
                            finish(lambda: accepted.append(True))

                    with patch("e2e.campaign.default_root", return_value=root / "admission"):
                        self.assertEqual(entry(work, name="scenario"), 1)
                    self.assertEqual(agent["task_outcome"], "cancelled")
                    self.assertEqual(services, ["closed"])
                    self.assertFalse(accepted)
                    self.assertEqual(read_receipt(root / "admission")["phase"], "starting")

    def test_actual_shutdown_and_late_acceptance_proof(self):
        from e2e.campaign import read_receipt

        for mode in ("normal", "fail", "ignore-child", "late"):
            with self.subTest(mode=mode), state_fixture() as root:
                atomic_write_bytes(root / "mode", mode.encode())

                class Call:
                    command = [sys.executable, str(FIXTURE), str(root)]

                    def argv(self, *args):
                        return list(args)

                    def __call__(self, *args):
                        if args[0] != "agents":
                            raise AssertionError("Only pre-shutdown reader snapshots are expected")
                        return dict(agents=[], next_after=None)

                def status(*argv, **options):
                    path = root / "ready.json"
                    value = (
                        json.loads(path.read_text())
                        if path.exists()
                        else dict(error=dict(code="daemon_unavailable"))
                    )
                    return subprocess.CompletedProcess(
                        argv, 0 if "pid" in value else 1, json.dumps(value), ""
                    )

                call = Call()
                owner = live.Daemon(call, root, root / "unused", root)
                args = SimpleNamespace(state_dir=root, output_dir=root)
                accepted = []

                def work():
                    with live.campaign(args, "fixture") as admission:
                        with finalizer(owner.close):
                            owner.start(admission)
                            if mode == "ignore-child":
                                owner.process.stop_grace = 0.05
                            live.verify_tasks(call)
                            live.verify_tasks(call)
                        admission.advance("cleaned")
                        finish(lambda: accepted.append(True))

                try:
                    with (
                        patch.object(live, "run", side_effect=status),
                        patch("e2e.campaign.default_root", return_value=root / "admission"),
                    ):
                        self.assertEqual(entry(work, name="scenario"), 0 if mode == "normal" else 1)
                    self.assertEqual(accepted, [True] if mode == "normal" else [])
                    self.assertEqual(read_receipt(root / "admission") is None, mode == "normal")
                    self.assertTrue(owner.process.completed)
                    self.assertEqual(
                        owner.process.returncode,
                        {"normal": 0, "fail": 2, "ignore-child": -9, "late": 0}[mode],
                    )
                    if mode == "late":
                        self.assertEqual(
                            json.loads((root / "retirement-failure.json").read_text()),
                            dict(kind="agent", identity="late", reason="task_open"),
                        )
                    if mode == "ignore-child":
                        self.assertTrue(
                            request("unix://" + str(root / "provider.sock"), ["ping"])["ok"]
                        )
                finally:
                    if (
                        mode == "ignore-child"
                        and (root / "provider-ready.json").exists()
                        and not (root / "provider-done.json").exists()
                    ):
                        try:
                            request("unix://" + str(root / "provider.sock"), ["quit"])
                            until = time.monotonic() + 5
                            while not (root / "provider-done.json").exists():
                                if time.monotonic() >= until:
                                    raise TimeoutError("Fixture provider failed to finish")
                                pause(0.01)
                        except BaseException as error:
                            error.owned_cleanup_uncertain = True
                            raise

    def test_every_run_provider_requires_its_own_durable_shutdown(self):
        good = dict(
            shutdown=dict(method="eof", confirmed=True, quiescent=True, reaped=True, exit_code=0),
            error_code=None,
        )
        for bad in (None, "confirmed", "quiescent", "reaped"):
            with (
                self.subTest(bad=bad),
                state_fixture() as state,
                sqlite3.connect(state / "runtime/runtime.sqlite") as database,
            ):
                database.execute(
                    "INSERT INTO provider_events(provider,at,kind,data) VALUES('owned',0,'started','{}')"
                )
                if bad is not None:
                    report = copy.deepcopy(good)
                    report["shutdown"][bad] = False
                    database.execute(
                        "INSERT INTO provider_events(provider,at,kind,data) VALUES('owned',0,'shutdown',?)",
                        (json.dumps(report),),
                    )
                with self.assertRaises(RuntimeError):
                    verify(database, Snapshot({}, 0, 0))
                database.execute(
                    "INSERT INTO provider_events(provider,at,kind,data) VALUES('owned',0,'shutdown',?)",
                    (json.dumps(good),),
                )
                verify(database, Snapshot({}, 0, 0))

    def test_terminal_stopped_fence_and_late_open_root_are_not_accepted(self):
        for data, outcome, thread in (
            (
                dict(conversation_clean=True, recovery_fence=dict(reason="cleanup")),
                "cancelled",
                "thread",
            ),
            (dict(conversation_clean=False), "completed", "thread"),
            (dict(conversation_clean=False), "open", None),
        ):
            with (
                self.subTest(data=data, outcome=outcome),
                state_fixture() as state,
                sqlite3.connect(state / "runtime/runtime.sqlite") as database,
            ):
                snapshot = Snapshot({}, 0, 0)
                database.execute(
                    "INSERT INTO agents(id,data,state,task_outcome,expired,thread_id,listing_seq) VALUES('late',?,'stopped',?,0,?,1)",
                    (json.dumps(data), outcome, thread),
                )
                with self.assertRaises(RuntimeError):
                    verify(database, snapshot)
                database.execute(
                    "UPDATE agents SET data=?,task_outcome='cancelled'",
                    (json.dumps(dict(conversation_clean=True)),),
                )
                verify(database, snapshot)

    def test_never_started_terminal_agent_needs_no_invented_session_release(self):
        with (
            state_fixture() as state,
            sqlite3.connect(state / "runtime/runtime.sqlite") as database,
        ):
            database.execute(
                "INSERT INTO agents(id,data,state,task_outcome,expired,thread_id,listing_seq) VALUES('unused',?,'stopped','cancelled',0,NULL,1)",
                (json.dumps(dict(conversation_clean=False)),),
            )
            verify(database, Snapshot({}, 0, 0))


class CleanupBudgetTests(unittest.TestCase):
    def test_slow_targets_share_one_absolute_budget_and_service_teardown_still_runs(self):
        clock = [0.0]
        timeouts = []
        agents = [
            dict(
                id=name,
                parent_id=None if name == "root" else "root",
                task_outcome="open",
                expired=False,
                state="idle",
            )
            for name in ("root", "a", "b", "c")
        ]
        call = cli.Cli(["fixture"], timeout=60)

        def native(argv, **options):
            timeouts.append(options["timeout"])
            cost = 0.2 if argv[1] == "agents" else 0.75
            clock[0] += min(cost, options["timeout"])
            if options["timeout"] < cost:
                raise subprocess.TimeoutExpired(argv, options["timeout"])
            if argv[1] == "agents":
                value = dict(agents=copy.deepcopy(agents), next_after=None)
            else:
                target = next(a for a in agents if a["id"] == argv[2])
                target.update(task_outcome="cancelled", state="stopped")
                value = {}
            return subprocess.CompletedProcess(argv, 0, json.dumps(value), "")

        closed = []
        with (
            patch.object(cli, "run_process", side_effect=native),
            patch.object(cli.time, "monotonic", side_effect=lambda: clock[0]),
        ):
            with self.assertRaises(TimeoutError), finalizer(lambda: closed.append(True)):
                live.cancel_descendants(call, "root", lambda: cli.read_agents(call), timeout=2)
        self.assertEqual(closed, [True])
        self.assertLessEqual(clock[0], 2)
        self.assertGreater(len(timeouts), 2)
        self.assertLess(timeouts[-1], timeouts[0])

    def test_pagination_cannot_restart_the_cleanup_timeout(self):
        clock = [0.0]
        timeouts = []

        def native(argv, **options):
            timeouts.append(options["timeout"])
            clock[0] += min(0.7, options["timeout"])
            if options["timeout"] < 0.7:
                raise subprocess.TimeoutExpired(argv, options["timeout"])
            return subprocess.CompletedProcess(
                argv, 0, json.dumps(dict(agents=[], next_after=len(timeouts))), ""
            )

        with (
            patch.object(cli, "run_process", side_effect=native),
            patch.object(cli.time, "monotonic", side_effect=lambda: clock[0]),
        ):
            with cli.call_deadline(2), self.assertRaises(subprocess.TimeoutExpired):
                cli.read_agents(cli.Cli(["fixture"], timeout=60))
        self.assertEqual(len(timeouts), 3)
        self.assertAlmostEqual(timeouts[-1], 0.6)
