"""The live verifier must reject false positives without spending model turns."""

import copy
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from e2e.scripts.comb_live_fixture import (
    make_facts,
    verify_retrieval,
    verify_corrected_fact,
    verify_relearning,
)
from e2e.scripts.run_live_comb import Scenario, main, read_json_output


class LiveCombVerification(unittest.TestCase):
    def test_explicit_root_binary_reaches_scenario_without_checkout(self):
        argv = [
            "e2e.scripts.run_live_comb.py",
            "--live",
            "--binary",
            "/tmp/release/hivectl",
            "--root-binary",
            "/tmp/release/comb",
            "--state-dir",
            "/tmp/state",
            "--output-dir",
            "/tmp/output",
            "--embedding-config",
            "/tmp/qwen.json",
        ]
        with (
            patch.dict("os.environ", {}, clear=True),
            patch.object(sys, "argv", argv),
            patch("e2e.scripts.run_live_comb.require_paths"),
            patch("e2e.scripts.run_live_comb.Scenario") as scenario,
        ):
            scenario.return_value.run.return_value = 0
            with self.assertRaises(SystemExit) as result:
                main()
        self.assertEqual(result.exception.code, 0)
        self.assertEqual(scenario.call_args.args[0].comb_binary, Path("/tmp/release/comb"))

    def test_live_startup_initializes_without_unused_socket(self):
        # Exercise startup through the actual local/remote prefixes, without
        # models, providers or sockets. Stop as soon as init is dispatched.
        class Initialized(Exception):
            pass

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = SimpleNamespace(
                output_dir=root / "output",
                comb_binary=Path("/tmp/release/comb"),
                binary=Path("/tmp/release/hivectl"),
                state_dir=root / "state",
                auth="mock",
                model="mock",
                effort="medium",
                full=False,
                embedding_config=root / "embedding.json",
            )
            scenario = Scenario(args)
            self.addCleanup(scenario.stack.close)

            def hb(*arguments):
                if arguments == ("init",):
                    raise Initialized()
                raise AssertionError(arguments)

            def probe(*arguments):
                if arguments[0] == "embed":
                    return dict(total=1, embedded=1, pending=0, rejected=0)
                if arguments[0] == "search":
                    return dict(fallback=None, mode="vector", results=[{}])
                return {}

            scenario.cli = Mock(
                side_effect=[
                    dict(state="authenticated"),
                    dict(
                        models=dict(
                            data=[
                                dict(
                                    model="mock",
                                    supportedReasoningEfforts=[dict(reasoningEffort="medium")],
                                )
                            ]
                        )
                    ),
                ]
            )
            scenario.hb_local = Mock(wraps=scenario.hb_local, side_effect=hb)
            with (
                patch("e2e.scripts.run_live_comb.preflight"),
                patch("e2e.scripts.run_live_comb.Cli", return_value=probe),
            ):
                with self.assertRaises(Initialized):
                    scenario.startup()
            scenario.hb_local.assert_called_once_with("init")
            local_args = scenario.hb_local.argv("init")
            self.assertNotIn("--socket", local_args)
            self.assertIn("--root", local_args)
            for verb in ("serve", "credential", "stats", "doctor", "search"):
                self.assertIn("--socket", scenario.hb.argv(verb))

    def setUp(self):
        self.facts = make_facts()
        self.answers = [
            dict(id=f["id"], slug=f["slug"], verification_code=f["code"]) for f in self.facts
        ]
        self.rows = []
        for fact in self.facts:
            self.rows.extend(
                [
                    dict(
                        start=1,
                        end=2,
                        claim=dict(id="reader"),
                        operation=dict(
                            op="search", mode="vector", request=dict(query=fact["query"], limit=3)
                        ),
                        reply=dict(
                            status="ok",
                            result=dict(
                                mode="vector",
                                fallback=None,
                                coverage=dict(total=20, embedded=20, pending=0, rejected=0),
                                results=[dict(slug=fact["slug"])],
                            ),
                        ),
                    ),
                    dict(
                        start=3,
                        end=4,
                        claim=dict(id="reader"),
                        operation=dict(op="get", slug=fact["slug"]),
                        reply=dict(status="ok", result={}),
                    ),
                ]
            )

    def verify(self, rows=None, answers=None):
        return verify_retrieval(
            self.facts,
            "reader",
            self.rows if rows is None else rows,
            self.answers if answers is None else answers,
            "vector",
        )

    def test_random_answers_and_successful_trace(self):
        self.assertNotEqual(self.facts[0]["code"], make_facts()[0]["code"])
        self.assertEqual(len(self.verify()), 10)

    def test_wrong_mode_fallback_missing_source_and_broad_scan_rejected(self):
        for change in ("mode", "fallback", "get", "rank", "limit", "identity", "duplicate"):
            with self.subTest(change=change):
                rows = copy.deepcopy(self.rows)
                if change == "mode":
                    rows[0]["operation"]["mode"] = "text"
                elif change == "fallback":
                    rows[0]["reply"]["result"]["fallback"] = "embeddings_busy"
                elif change == "get":
                    rows.pop(1)
                elif change == "rank":
                    rows[0]["reply"]["result"]["results"] = []
                elif change == "limit":
                    rows[0]["operation"]["request"]["limit"] = 100
                elif change == "identity":
                    rows[0]["claim"]["id"] = "different-reader"
                elif change == "duplicate":
                    rows.append(copy.deepcopy(rows[0]))
                with self.assertRaises(AssertionError):
                    self.verify(rows)

    def test_guessed_or_incomplete_answers_rejected(self):
        answers = copy.deepcopy(self.answers)
        answers[0]["verification_code"] = "guessed"
        with self.assertRaises(AssertionError):
            self.verify(answers=answers)
        with self.assertRaises(AssertionError):
            self.verify(answers=self.answers[:-1])

    def test_stale_answer_rejected_after_correction(self):
        self.facts[0]["code"] = make_facts()[0]["code"]
        with self.assertRaises(AssertionError):
            self.verify()
        self.answers[0]["verification_code"] = self.facts[0]["code"]
        self.assertEqual(len(self.verify()), 10)

    def test_corrected_claims_allow_incidental_prose_but_reject_old_fact(self):
        with self.assertRaises(AssertionError):
            verify_corrected_fact(self.facts[0]["fact"])
        verify_corrected_fact(
            "The hillside observatory keeps its telescope tracking during electrical outages "
            "using a hydrogen fuel cell. The flywheel energy store was retired."
        )

    def test_learning_allows_one_canonical_read_after_both_searches(self):
        fact = self.facts[0]
        vector = copy.deepcopy(self.rows[0])
        text = copy.deepcopy(vector)
        text.update(start=3, end=4)
        text["operation"]["mode"] = text["reply"]["result"]["mode"] = "text"
        text["operation"]["request"]["query"] = fact["marker"]
        read = copy.deepcopy(self.rows[1])
        read.update(start=5, end=6)
        current = dict(
            self.answers[0],
            fact="Backup runs using a hydrogen fuel cell. The flywheel energy store was retired.",
        )
        answers = dict(vector=[current], text=[current])
        self.assertEqual(len(verify_relearning(fact, "reader", [vector, text, read], answers)), 2)
        for defect in ("inbox_only", "read_too_early", "old_code", "old_fact"):
            with self.subTest(defect=defect):
                rows, answer = copy.deepcopy([vector, text, read]), copy.deepcopy(answers)
                if defect == "inbox_only":
                    rows[-1]["operation"]["slug"] = "inbox/correction"
                elif defect == "read_too_early":
                    rows[-1].update(start=2.1, end=2.2)
                elif defect == "old_code":
                    answer["vector"][0]["verification_code"] = "old"
                elif defect == "old_fact":
                    answer["vector"][0]["fact"] = fact["fact"]
                with self.assertRaises(AssertionError):
                    verify_relearning(fact, "reader", rows, answer)

    def test_reads_public_assistant_message_event(self):
        events = [
            dict(type="turn_finished", data={}),
            dict(type="assistant_message", data=dict(text='{"answers": [1]}')),
        ]
        self.assertEqual(read_json_output(events), {"answers": [1]})
        with self.assertRaises(AssertionError):
            read_json_output(events[:1])

    def test_idle_before_queued_message_runs_is_not_finished(self):
        old = dict(
            id="reader",
            state="idle",
            task_outcome="open",
            turn_id=None,
            last_finished_turn=dict(message="old"),
        )
        new = dict(old, last_finished_turn=dict(message="new"))
        scenario = SimpleNamespace(
            args=SimpleNamespace(timeout=10),
            peak_running=0,
            proxy=None,
            initial_messages={},
            cli=Mock(side_effect=[{"agent": old}, {"agent": new}]),
        )
        with patch("e2e.scripts.run_live_comb.pause"), patch("e2e.scripts.run_live_comb.progress"):
            result = Scenario.wait(scenario, ["reader"], complete=False, message="new")
        self.assertEqual(result, [new])
        self.assertEqual(scenario.cli.call_count, 2)

    def test_initial_idle_does_not_complete_the_spawn_message(self):
        old = dict(
            id="writer", state="idle", task_outcome="open", turn_id=None, last_finished_turn=None
        )
        new = dict(old, last_finished_turn=dict(message="initial"))
        scenario = SimpleNamespace(
            args=SimpleNamespace(timeout=10),
            peak_running=0,
            proxy=None,
            initial_messages={"writer": "initial"},
            cli=Mock(side_effect=[{"agent": old}, {"agent": new}]),
        )
        with patch("e2e.scripts.run_live_comb.pause"), patch("e2e.scripts.run_live_comb.progress"):
            self.assertEqual(Scenario.wait(scenario, ["writer"], complete=False), [new])
        self.assertFalse(scenario.initial_messages)
        self.assertEqual(scenario.cli.call_count, 2)

    def test_live_opt_in_and_default_workload_and_caps(self):
        from e2e.scripts.run_live_comb import arguments

        for extra, count, model, effort, timeout in (
            ([], 1, "gpt-6.1-terra", "low", 600),
            (["--full"], 12, "gpt-6.1-terra", "medium", 3600),
        ):
            with (
                patch.object(sys, "argv", ["comb", "--live", *extra]),
                patch("e2e.scripts.run_live_comb.require_paths"),
                patch("e2e.scripts.run_live_comb.opt_in") as opt,
            ):
                args = arguments()
            self.assertEqual((args.model, args.effort, args.timeout), (model, effort, timeout))
            self.assertEqual(opt.call_args.kwargs["agents"], count)
        for extra in ([], ["--live", "--timeout", "601"], ["--live", "--timeout", "0"]):
            with (
                patch.object(sys, "argv", ["comb", *extra]),
                patch("e2e.scripts.run_live_comb.require_paths") as paths,
                patch("e2e.scripts.run_live_comb.Scenario") as scenario,
            ):
                with self.assertRaises(SystemExit):
                    main()
                paths.assert_not_called()
                scenario.assert_not_called()

    def test_fixed_collector_name_is_reused_and_small_mode_has_none(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for index, full in enumerate((False, True, True)):
                args = SimpleNamespace(
                    output_dir=root / str(index),
                    comb_binary=Path("/release/comb"),
                    binary=Path("/release/hivectl"),
                    state_dir=root / "state",
                    auth="mock",
                    model="mock",
                    effort="medium",
                    full=full,
                )
                scenario = Scenario(args)
                try:
                    config = __import__("json").loads(scenario.config.read_text())
                    self.assertEqual(len(scenario.facts), 10 if full else 1)
                    self.assertEqual(
                        set(config.get("comb_collectors", {})), {"hive-e2e"} if full else set()
                    )
                finally:
                    scenario.stack.close()

    def test_rejected_evidence_is_always_a_failure(self):
        for row in self.rows[::2]:
            row["reply"]["result"]["coverage"]["rejected"] = 1
            row["reply"]["result"]["coverage"]["embedded"] = 19
        with self.assertRaises(AssertionError):
            self.verify()

    def test_cleanup_attempts_both_services_when_hosted_cleanup_fails(self):
        from release.progress import entry

        calls = []
        scenario = object.__new__(Scenario)
        scenario.daemon_owner = SimpleNamespace(ready=True, close=lambda: calls.append("daemon"))
        scenario.proxy = SimpleNamespace(stop=lambda: calls.append("proxy"))
        scenario.comb_process = SimpleNamespace(stop=lambda: calls.append("comb"), returncode=0)
        scenario.cli, scenario.agents = Mock(), ["root"]
        scenario.discover = Mock(return_value=[])
        scenario.retain_agent_evidence = lambda: calls.append("evidence")
        scenario.discard_collector = lambda: calls.append("discard")
        scenario.pending_spawns = set()
        scenario.admission = Mock()
        with (
            patch(
                "e2e.scripts.run_live_comb.cancel_descendants", side_effect=RuntimeError("fixture")
            ),
            patch("e2e.scripts.run_live_comb.verify_tasks"),
        ):
            self.assertEqual(entry(scenario.cleanup, name="scenario"), 1)
        self.assertEqual(calls, ["evidence", "discard", "daemon", "proxy", "comb"])
        scenario.admission.advance.assert_not_called()

    def test_failed_comb_shutdown_does_not_clear_admission(self):
        from release.progress import entry

        scenario = object.__new__(Scenario)
        scenario.daemon_owner = SimpleNamespace(ready=False, close=Mock())
        scenario.proxy = None
        scenario.comb_process = SimpleNamespace(stop=Mock(), returncode=2)
        scenario.pending_spawns = set()
        scenario.admission = Mock()
        self.assertEqual(entry(scenario.cleanup, name="scenario"), 1)
        scenario.daemon_owner.close.assert_called_once()
        scenario.comb_process.stop.assert_called_once()
        scenario.admission.advance.assert_not_called()

    def test_discard_native_failure_prints_exact_recovery_without_exception_text(self):
        import contextlib
        import io
        import json

        scenario = object.__new__(Scenario)
        scenario.collector, scenario.collector_bound, scenario.comb_id = "hive-e2e", True, "owned"
        scenario.args = SimpleNamespace(state_dir=Path("/state"), binary=Path("/release/hivectl"))
        scenario.private, scenario.output = Path("run"), Path("unused")
        scenario.collector_status = Mock(return_value=dict(comb_id="owned"))
        scenario.cli = Mock(side_effect=TimeoutError("private cause is not displayed"))
        with (
            patch("e2e.scripts.run_live_comb.save"),
            contextlib.redirect_stdout(io.StringIO()) as output,
        ):
            with self.assertRaises(TimeoutError):
                scenario.discard_collector()
        recovery = json.loads(output.getvalue())["collector_recovery"]
        self.assertEqual(
            recovery["inspect"],
            ["/release/hivectl", "--state-dir", "/state", "collectors", "status"],
        )
        self.assertEqual(
            recovery["discard_after_inspection"][:7],
            [
                "/release/hivectl",
                "--state-dir",
                "/state",
                "collectors",
                "discard",
                "hive-e2e",
                "--request-id",
            ],
        )
        self.assertIn("daemon", recovery["prerequisite"])
        self.assertNotIn("private cause", output.getvalue())

    def test_admission_refusal_still_retires_private_temporary_root(self):
        from release.progress import entry

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = SimpleNamespace(
                output_dir=root / "output",
                comb_binary=Path("/release/comb"),
                binary=Path("/release/hivectl"),
                state_dir=root / "state",
                auth="codex-main",
                model="gpt-6-luna",
                effort="medium",
                full=False,
                timeout=600,
            )
            scenario = Scenario(args)
            private = scenario.private
            self.assertTrue(private.exists())
            with (
                patch("e2e.scripts.run_live_comb.campaign", side_effect=RuntimeError("busy")),
                patch.object(scenario, "startup") as startup,
            ):
                self.assertEqual(entry(scenario.run, name="scenario"), 1)
            startup.assert_not_called()
            self.assertFalse(private.exists())
            self.assertFalse((scenario.output / "result.json").exists())

    def test_reused_empty_collector_is_bound_to_this_runs_comb(self):
        s = object.__new__(Scenario)
        s.collector = "hive-e2e"
        s.args = SimpleNamespace(state_dir=Path("/state"), binary=Path("/release/hivectl"))
        s.private, s.output = Path("run-id"), Path("unused")
        old = dict(name=s.collector, pending=0, uncertain=0, gap_count=0, comb_id="old")
        s.cli = Mock(
            side_effect=[{"collectors": [old]}, {}, {"collectors": [dict(old, comb_id="new")]}]
        )
        with patch("e2e.scripts.run_live_comb.save"):
            s.prepare_collector("new")
        self.assertEqual(
            s.cli.call_args_list[1].args,
            ("collectors", "rebind", s.collector, "new", "--request-id", "run-id-bind-comb"),
        )
        for field in ("pending", "uncertain", "gap_count"):
            s.cli = Mock(return_value={"collectors": [dict(old, **{field: 1})]})
            with self.assertRaises(RuntimeError):
                s.prepare_collector("new")
            self.assertEqual(s.cli.call_count, 1, "Must not rebind retained evidence")


if __name__ == "__main__":
    unittest.main()
