"""Harness ownership and evidence checks; these never start a native provider."""

import collections
import json
import sqlite3
import unittest
from contextlib import closing
from unittest.mock import Mock


from e2e.scripts import run_conversation_controls as controls
from e2e.tests.test_live_ownership import state_fixture

from release.files import atomic_write_text
from e2e.scripts.verify_ping_pong_pair import verify_terminal_cleanup


class Harness(unittest.TestCase):
    def setUp(self):
        self.fixture = state_fixture()
        root = self.fixture.__enter__()
        self.addCleanup(self.fixture.__exit__, None, None, None)
        self.run = controls.Run.__new__(controls.Run)
        self.run.root = self.run.output = root
        self.run.results, self.run.pending = {}, {}
        self.run.agents = []
        self.run.roots = self.run.agents
        self.run.spawn_intents = []
        self.run.cleanup_until = None

    def test_prior_agents_rejected_without_modification(self):
        path = self.run.root / "runtime/runtime.sqlite"
        with closing(sqlite3.connect(path)) as db, db:
            db.execute(
                "INSERT INTO agents(id,state,task_outcome,expired) VALUES('old','stopped','open',0)"
            )
        before = path.read_bytes()
        with self.assertRaisesRegex(RuntimeError, "unfinished agents"):
            self.run.preflight()
        self.assertEqual(before, path.read_bytes())

    def test_fresh_controller_preflight_leaves_runtime_creation_to_product(self):
        runtime = self.run.root / "runtime"
        (runtime / "daemon.lock").unlink()
        (runtime / "runtime.sqlite").unlink()
        runtime.rmdir()
        self.assertEqual(self.run.preflight().paused, {})
        self.assertFalse(runtime.exists())

    def test_preflight_does_not_manufacture_missing_ownership_lock(self):
        path = self.run.root / "runtime/daemon.lock"
        path.unlink()
        with self.assertRaises(ValueError):
            self.run.preflight()
        self.assertFalse(path.exists())

    def test_terminal_history_can_be_retained_for_rerun(self):
        path = self.run.root / "runtime/runtime.sqlite"
        with closing(sqlite3.connect(path)) as db, db:
            db.execute(
                "INSERT INTO agents(id,state,task_outcome,expired,listing_seq) VALUES('old','stopped','cancelled',0,1)"
            )
        self.assertEqual(self.run.preflight().agent_after, 1)

    def test_mailbox_cursor_on_last_nonempty_page_is_normal(self):
        self.run.call = Mock(
            side_effect=[
                {"messages": [{"id": "m"}], "next_before": 1},
                {"messages": [], "next_before": None},
            ]
        )
        self.assertEqual(self.run.messages("a"), [{"id": "m"}])
        self.assertEqual(self.run.call.call_args.kwargs["before"], 1)

    def test_short_event_page_does_not_hide_later_events(self):
        self.run.call = Mock(
            side_effect=[
                {"events": [{"seq": 1}], "cursor": 1},
                {"events": [{"seq": 2}], "cursor": 2},
                {"events": [], "cursor": 2},
            ]
        )
        self.assertEqual(self.run.events("a"), [{"seq": 1}, {"seq": 2}])

    def test_pair_cleanup_evidence_rejects_wrong_binding_and_order(self):
        def events(execution="e", sequence=2):
            return collections.defaultdict(
                list,
                turn_finished=[
                    dict(seq=1, execution="e", data=dict(turn_id="t", status="interrupted"))
                ],
                terminal_cleanup_observed=[
                    dict(
                        seq=sequence,
                        execution=execution,
                        data=dict(
                            turn_id="t",
                            scope="conversation",
                            remaining=0,
                            next_cursor=None,
                            observation=dict(
                                kind="observed_quiet",
                                quiet_ms=3000,
                                elapsed_ms=3100,
                                deadline_ms=20000,
                                recleans=1,
                                late_items_tolerated=1,
                            ),
                        ),
                    )
                ],
                turn_started=[dict(seq=3)],
            )

        verify_terminal_cleanup(events())
        for bad, expected in [
            (events(execution="wrong"), "Expected one cleanup turn, got 0"),
            (events(sequence=0), "Cleanup predates turn completion"),
            (events(sequence=4), "Turn admitted before terminal cleanup"),
        ]:
            with self.assertRaisesRegex(RuntimeError, expected):
                verify_terminal_cleanup(bad)
        for field, value in [
            ("kind", "confirmed"),
            ("quiet_ms", 2999),
            ("elapsed_ms", 20001),
            ("deadline_ms", 30000),
            ("quiet_ms", 5000),
            ("quiet_ms", 3000.0),
            ("elapsed_ms", True),
            ("recleans", -1),
            ("late_items_tolerated", True),
        ]:
            bad = events()
            bad["terminal_cleanup_observed"][0]["data"]["observation"][field] = value
            with self.assertRaisesRegex(RuntimeError, "Invalid bounded quiet observation"):
                verify_terminal_cleanup(bad)

    def test_unknown_database_is_not_treated_as_empty(self):
        atomic_write_text(self.run.root / "runtime/runtime.sqlite", "broken")
        with self.assertRaises(sqlite3.DatabaseError):
            self.run.preflight()

    def test_spawn_timeout_reuses_persisted_envelope(self):
        calls = []

        def call(op, **args):
            persisted = json.loads((self.run.output / "observations.json").read_text())
            self.assertEqual(persisted["spawn_intents"][0]["request_id"], args["request_id"])
            calls.append((op, args))
            if len(calls) == 1:
                raise TimeoutError("accepted but response lost")
            return {"agent_id": "owned"}

        self.run.call = call
        with self.assertRaises(TimeoutError):
            self.run.spawn(role="codex", cwd="/test", body="original")
        self.run.resolve_spawn(self.run.spawn_intents[0])
        self.assertEqual(calls[0], calls[1])
        self.assertEqual(self.run.agents, ["owned"])

    def test_wrong_steering_execution_is_rejected(self):
        self.run.call = Mock(
            return_value={
                "message_id": "m",
                "steering_target": {"execution_id": "other", "turn_id": "t"},
            }
        )
        with self.assertRaisesRegex(RuntimeError, "Steering target changed"):
            self.run.steer("a", {"execution_id": "expected", "turn_id": "t"}, "guide")

    def test_gate_reports_early_completion_without_waiting_for_timeout(self):
        self.run.workspace = self.run.root
        self.run.send = Mock(return_value="m")
        self.run.status = Mock()
        self.run.messages = Mock(return_value=[{"id": "m", "state": "completed"}])
        with self.assertRaisesRegex(RuntimeError, "Gate turn ended before helper readiness"):
            self.run.gate("a", "declined")

    def test_cleanup_requires_confirmed_release_then_execution_stop(self):
        start = dict(seq=1, type="provider_started", execution_id="e")
        release = dict(
            seq=2,
            type="provider_shutdown",
            execution_id="e",
            data=dict(confirmed=True, quiescent=True),
        )
        stopped = dict(seq=3, type="execution_stopped", execution_id="e")
        controls.assert_clean_executions([start, release, stopped])
        for events, diagnostic in (
            ([start, stopped], "Missing provider start/release evidence"),
            ([release], "Missing provider start/release evidence"),
            ([start, release, release, stopped], "Duplicate provider release"),
            (
                [start, dict(release, data=dict(confirmed=False, quiescent=True)), stopped],
                "Provider cleanup unconfirmed",
            ),
            (
                [start, dict(release, data=dict(confirmed=True, quiescent=False)), stopped],
                "Provider cleanup unconfirmed",
            ),
            ([start, release], "Missing execution ending evidence"),
            ([start, dict(release, seq=4), stopped], "Invalid execution release order"),
        ):
            with self.assertRaisesRegex(RuntimeError, diagnostic):
                controls.assert_clean_executions(events)

    def test_pooled_stop_resume_releases_sessions_before_process_shutdown(self):
        rows = []
        for i, execution in enumerate(("first", "resumed")):
            rows += [
                dict(
                    seq=3 * i + 1,
                    type="provider_started",
                    execution_id=execution,
                    data=dict(pid=7, provider_id="shared"),
                ),
                dict(
                    seq=3 * i + 2,
                    type="provider_session_released",
                    execution_id=execution,
                    data=dict(
                        confirmed=True, quiescent=True, reaped=False, method="session_released"
                    ),
                ),
                dict(seq=3 * i + 3, type="execution_stopped", execution_id=execution),
            ]
        controls.assert_clean_executions(rows)
        physical = dict(
            seq=7,
            type="provider_shutdown",
            execution_id="resumed",
            data=dict(confirmed=True, quiescent=True, reaped=True),
        )
        controls.assert_clean_executions([*rows, physical])
        with self.assertRaisesRegex(RuntimeError, "Duplicate provider release"):
            controls.assert_clean_executions([*rows, rows[-2]])

    def test_unexpected_terminal_message_state_fails_without_timeout(self):
        self.run.status = Mock()
        self.run.messages = Mock(return_value=[dict(id="m", state="completed")])
        with self.assertRaisesRegex(RuntimeError, "expected interrupted"):
            self.run.delivered("a", "m", "interrupted")

    def test_no_tool_redirect_completion_race_is_inconclusive(self):
        self.run.results = dict(inconclusive=[])
        self.run.send = Mock(return_value="m")
        self.run.status = Mock(return_value=dict(turn_id=None))
        self.run.messages = Mock(return_value=[dict(id="m", state="completed")])
        self.run.call = Mock()
        self.run.no_tool_redirect("a", "thread")
        self.assertEqual(len(self.run.results["inconclusive"]), 1)
        self.run.call.assert_not_called()

    def test_stale_guidance_leaking_into_later_output_is_rejected(self):
        self.run.delivered = Mock()
        self.run.text = Mock(side_effect=["interrupted", "LEAK"])
        self.run.events = Mock(return_value=[])
        self.run.send = Mock(return_value="next")
        with self.assertRaisesRegex(RuntimeError, "Stale guidance leaked into later output"):
            self.run.assert_stale("a", "original", {"message_id": "guide"}, "LEAK")

    def test_stale_guidance_with_success_receipt_is_rejected(self):
        self.run.delivered = Mock()
        self.run.text = Mock(return_value="interrupted")
        self.run.events = Mock(
            return_value=[{"type": "steering_submitted", "data": {"message_id": "guide"}}]
        )
        with self.assertRaisesRegex(RuntimeError, "Stale guidance has successful receipt"):
            self.run.assert_stale("a", "original", {"message_id": "guide"}, "marker")


if __name__ == "__main__":
    unittest.main()
