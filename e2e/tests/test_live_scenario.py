"""Live ownership and report decisions tested without provider or model calls."""

import json
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from e2e.campaign import read_receipt
from e2e.live.scenario import Hosted
from e2e.scripts import run_conversation_controls as controls
from e2e.tests.test_live_ownership import state_fixture
from release.files import atomic_write_text
from release.process import run
from release.progress import entry


class LiveScenarioTests(unittest.TestCase):
    def test_cleanup_failure_and_unknown_spawn_never_publish_or_clear_admission(self):
        for failure in (None, "work", "cleanup", "unknown-spawn"):
            with self.subTest(failure=failure), state_fixture() as state:
                args = SimpleNamespace(
                    output_dir=state / "out",
                    binary=Path("fixture/release/hivectl"),
                    state_dir=state,
                    timeout=10,
                )
                scenario = Hosted(args, label="fixture", roles={})
                calls = []

                def start(admission):
                    admission.advance("starting")
                    calls.append("start")
                    return {}

                def close():
                    calls.append("close")
                    if failure == "cleanup":
                        raise RuntimeError("Fixture shutdown failure")

                def execute():
                    calls.append("execute")
                    if failure == "work":
                        raise RuntimeError("Fixture primary failure")
                    if failure == "unknown-spawn":
                        scenario.pending["lost"] = dict(role="fixture")
                    return {}

                scenario.owner = SimpleNamespace(ready=False, start=start, close=close)
                scenario.prerequisites = Mock()
                scenario.execute = execute
                with patch("e2e.campaign.default_root", return_value=state / "admission"):
                    status = entry(scenario.run, name="scenario")
                self.assertEqual(status, 0 if failure is None else 1)
                self.assertEqual(calls, ["start", "execute", "close"])
                self.assertEqual((scenario.output / "result.json").exists(), failure is None)
                self.assertEqual(
                    read_receipt(state / "admission") is None, failure in (None, "work")
                )

    def test_spawn_intent_is_durable_before_a_lost_reply_and_is_not_replayed(self):
        with state_fixture() as state:
            args = SimpleNamespace(
                output_dir=state / "out", binary=Path("fixture/release/hivectl"), state_dir=state
            )
            scenario = Hosted(args, label="fixture", roles={})

            def lost(*command):
                pending = json.loads((scenario.output / "pending-spawns.json").read_text())
                self.assertEqual(len(pending), 1)
                self.assertIn(command[command.index("--request-id") + 1], pending)
                raise TimeoutError("Lost native acknowledgment")

            scenario.cli = Mock(side_effect=lost)
            with self.assertRaises(TimeoutError):
                scenario.spawn_task("fixture", "original body")
            self.assertEqual(scenario.cli.call_count, 1)
            self.assertEqual(len(scenario.pending), 1)
            self.assertFalse(scenario.roots)

    def test_shop_rejects_jointly_wrong_stored_and_native_selection(self):
        from e2e.scripts.run_shop_team import ShopTeam, LEAD
        from e2e.scripts.product_constants import LIVE_CLAUDE_MODEL

        scenario = object.__new__(ShopTeam)
        scenario.args = SimpleNamespace(
            model="gpt-6.1-terra",
            claude_model=LIVE_CLAUDE_MODEL,
            codex_auth="codex-main",
            claude_auth="claude-main",
            coordinator_effort="medium",
            worker_effort="medium",
        )
        for name in (LEAD, "restock fixer", "search builder", "claude reviewer"):
            claude = name == "claude reviewer"
            current = dict(
                id="agent",
                provider="claude" if claude else "codex",
                auth="claude-main" if claude else "codex-main",
                model=LIVE_CLAUDE_MODEL if claude else "gpt-6.1-terra",
                effort="low" if claude else "medium",
            )

            def native():
                return [
                    dict(
                        type="session_ready",
                        data=dict(model=current["model"], effort=current["effort"]),
                    )
                ]

            scenario.events = Mock(side_effect=lambda _: native())
            scenario.verify_selection(current, name)
            for field in ("model", "effort", "auth", "provider"):
                old = current[field]
                current[field] = "unrequested"
                with self.subTest(name=name, field=field), self.assertRaises(RuntimeError):
                    scenario.verify_selection(current, name)
                current[field] = old

    def test_commit_abort_is_saved_and_successful_histories_are_not_read_twice(self):
        from e2e.scripts.run_commit_review_demo import CommitReviewRun

        with state_fixture() as state:
            scenario = object.__new__(CommitReviewRun)
            scenario.manifest_path, scenario.output = state / "manifest.json", state
            scenario.manifest = dict(repo=str(state))
            scenario.spawn_task = Mock(return_value=dict(agent_id="root"))
            scenario.wait = Mock(side_effect=RuntimeError("Fixture RSS abort"))
            scenario.failure, scenario.evidence = None, None
            with self.assertRaises(RuntimeError):
                scenario.execute()
            self.assertEqual(scenario.failure, "RuntimeError")
            scenario.descendants = Mock(return_value=[])
            scenario.call = Mock(return_value=dict(messages=[], next_before=None))
            scenario.collect()
            self.assertEqual(
                json.loads((state / "evidence.json").read_text())["failure"], "RuntimeError"
            )
            scenario.failure = None
            scenario.collect = Mock()
            scenario.discover = Mock(return_value=[])
            scenario.retain()
            scenario.collect.assert_not_called()
            scenario.discover.assert_called_once()

    def test_claude_controls_keep_native_low_effort_and_haiku_low_change(self):
        with state_fixture() as state:
            args = SimpleNamespace(
                output_dir=state / "out",
                state_dir=state,
                binary=Path("fixture/release/hivectl"),
                provider="both",
                codex_model="gpt-6.1-terra",
                claude_model="sonnet",
            )
            scenario = controls.Run(args)
            config = json.loads(scenario.config.read_text())
            self.assertEqual(config["roles"]["codex"]["effort"], "low")
            self.assertEqual(config["roles"]["claude"]["effort"], "low")

    def test_codex_controls_restore_terra_after_the_model_change(self):
        scenario = controls.Run.__new__(controls.Run)
        scenario.workspace = Path("/fixture")
        scenario.results = {"providers": {}}
        scenario.spawn = Mock(return_value="agent")
        scenario.messages = Mock(return_value=[{"id": "first"}])
        scenario.delivered = Mock()
        scenario.status = Mock(return_value={"thread_id": "thread"})
        scenario.interrupt_trials = Mock()
        scenario.change = Mock(side_effect=[{"desired": {"revision": 1}},
                                            {"desired": {"revision": 2}}])
        scenario.remember = Mock(side_effect=[
            {"model": "gpt-6-sol", "effort": "medium", "options_revision": 1},
            {"model": "gpt-6.1-terra", "effort": "low", "options_revision": 2},
        ])
        scenario.resume = Mock(return_value={})
        scenario.gate = Mock(return_value=("message", {}, []))
        scenario.call = Mock()
        scenario.terminal_cleanup_observed = Mock(return_value={})
        scenario.gone = Mock()
        scenario.redirect = Mock(return_value=({}, [], {}))
        scenario.save = Mock()
        scenario.provider("codex", "gpt-6.1-terra", "gpt-6-sol", "medium")
        self.assertEqual(
            [call.args for call in scenario.change.call_args_list],
            [("agent", "gpt-6-sol", "medium"), ("agent", "gpt-6.1-terra", "low")],
        )

    def test_native_gate_fixture_finishes_and_reaps_its_child(self):
        with state_fixture() as state:
            atomic_write_text(state / "gate.release", "")
            value = run(
                [sys.executable, "-c", controls.GATE, "gate"],
                cwd=state,
                timeout=10,
                check=True,
                text=True,
            )
            self.assertIn("gate finished", value.stdout)
            self.assertTrue((state / "gate.ready").is_file())
