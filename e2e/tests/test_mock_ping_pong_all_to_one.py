"""Eviction and high-residency mock runs plus independent verifier corruption checks."""

from e2e.scripts.harness.scenario import run as run_process

import copy
import io
import json
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from contextlib import closing, redirect_stderr
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
from e2e.paths import product_root
from e2e.scripts.run_mock_ping_pong_all_to_one import (
    AllToOneRun,
    parse_args,
    runtime_limits,
    verify_database,
    verify_traces,
)


def corrupt_trace_event(event, kind):
    if kind == "conversation" and event["kind"] in ("thread_start", "thread_resume"):
        event["agent"] = "unrelated-agent"
    if kind == "turn" and event["kind"] == "turn_start":
        event["turn"] = "unrelated-turn"
    if kind == "message" and event["kind"] == "turn_start":
        event["message"] = "unrelated-message"
    if kind == "completion" and event["kind"] == "turn_end":
        event["turn"] = "wrong-completed-turn"
    if kind == "provider":
        # Preserve each process's lifetime identity so binding to runtime evidence fails.
        event["pid"] += 1000000
    if kind == "barrier" and event["kind"] == "barrier":
        event["overlapped"] = False


class MockScenarioTestCase(unittest.TestCase):
    runtime_args = []

    @classmethod
    def setUpClass(cls):
        if not (product_root() / "target/release/hivectl").exists():
            raise unittest.SkipTest(
                "Run cargo build --release --locked before mock integration checks"
            )
        cls.temporary = tempfile.TemporaryDirectory(prefix="hl-mock-check-")
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.output = Path(cls.temporary.name) / "run"
        preset_args = ["--preset", cls.preset] if cls.preset else []
        result = run_process(
            [
                sys.executable,
                str(ROOT / "run.py"),
                "mock",
                "all-to-one",
                "--agents",
                "100",
                "--output-dir",
                str(cls.output),
                "--timeout",
                "120",
            ]
            + preset_args
            + cls.runtime_args,
            capture_output=True,
            text=True,
            timeout=240,
        )
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)
        cls.summary = json.loads((cls.output / "verification.json").read_text())
        cls.root = json.loads((cls.output / "checkpoint.json").read_text())["root_id"]
        cls.manifest = json.loads((cls.output / "identity-manifest.json").read_text())
        cls.traces = list((cls.output / "mock-traces").glob("*.jsonl"))


class MockPingPongTests(MockScenarioTestCase):
    preset = "eviction"
    # With multiple providers the coordinator can stay resident while finished
    # children are evicted, so a small run need not resume any conversation.
    # One provider forces the coordinator's process to recycle before all 100
    # agents fit in 64 slots, proving resume without depending on scheduling.
    runtime_args = ["--max-providers", "1"]

    def test_hundred_agents_exchange_recycle_overlap_and_clean_up(self):
        self.assertEqual(self.summary["result"], "passed")
        self.assertEqual(self.summary["messages"], 397)
        self.assertEqual(self.summary["native_conversations"], 100)
        self.assertEqual(self.summary["explicit_cancellations"], 99)
        self.assertGreater(self.summary["resumed_agents"], 0)
        self.assertEqual(self.summary["limits"]["max_providers"], 1)
        self.assertEqual(self.summary["limits"]["max_loaded"], 64)
        root_thread = next(
            thread for thread, agent in self.manifest["threads"].items() if agent == self.root
        )
        self.assertTrue(
            any(
                thread == root_thread and resumed
                for _, thread, resumed in self.manifest["sessions"]
            )
        )
        self.assertGreaterEqual(self.summary["peak_same_process_turns"], 2)
        self.assertLessEqual(self.summary["peak_resident_threads"], 64)

    def test_corrupt_runtime_evidence_is_rejected(self):
        mutations = [
            "DELETE FROM messages WHERE seq=(SELECT max(seq) FROM messages)",
            "UPDATE messages SET state='accepted' WHERE body='received pong'",
            "UPDATE events SET data=json_set(data,'$.sender_turn_id','wrong') WHERE kind='message_accepted'",
            "UPDATE events SET data=json_set(data,'$.by','human') WHERE kind='termination_requested'",
            "UPDATE events SET data=json_set(data,'$.effort','high') WHERE kind='session_ready'",
            "UPDATE events SET data=json_set(data,'$.parent_id','human') WHERE kind='spawned' AND json_extract(data,'$.parent_id') IS NOT NULL",
            "UPDATE events SET data=json_set(data,'$.effort','high') WHERE kind='turn_started'",
            "UPDATE events SET data=json_set(data,'$.thread_id','unrelated') WHERE kind='session_ready'",
            "UPDATE provider_events SET data=json_set(data,'$.shutdown.confirmed',json('false')) WHERE kind='shutdown'",
            "INSERT INTO events(agent,execution,at,kind,data) SELECT agent,execution,at,kind,data FROM events WHERE kind='turn_started' LIMIT 1",
        ]
        diagnostics = [
            "Wrong exact message count",
            "Invalid delivery lifecycle",
            "Message was not sent from the expected input turn",
            "Wrong cancellation actor",
            "Conversation/model changed",
            "Child was not spawned in the root initial turn",
            "Turn audit changed model/effort",
            "Conversation/model changed",
            "Unconfirmed provider shutdown",
            "Message delivered more than once",
        ]
        for index, (mutation, diagnostic) in enumerate(zip(mutations, diagnostics, strict=True)):
            with self.subTest(mutation=mutation):
                path = Path(self.temporary.name) / f"bad-{index}.sqlite"
                shutil.copyfile(self.output / "runtime.sqlite", path)
                with closing(sqlite3.connect(path)) as db:
                    db.execute(mutation)
                    db.commit()
                with self.assertRaisesRegex(RuntimeError, diagnostic):
                    verify_database(path, self.root, 100)

    def test_corrupt_trace_identity_completion_and_overlap_are_rejected(self):
        original = {
            path.name: [json.loads(line) for line in path.read_text().splitlines()]
            for path in self.traces
        }
        for kind, diagnostic in (
            ("conversation", "Wrong trace conversation"),
            ("turn", "Wrong trace turn identity"),
            ("message", "Wrong trace turn identity"),
            ("completion", "Wrong trace turn completion"),
            ("provider", "Provider process identity mismatch"),
            ("barrier", "No verified overlap"),
        ):
            with self.subTest(kind=kind):
                directory = Path(self.temporary.name) / f"bad-{kind}"
                directory.mkdir()
                paths = []
                for name, frames in original.items():
                    changed = copy.deepcopy(frames)
                    for event in changed:
                        corrupt_trace_event(event, kind)
                    path = directory / name
                    paths.append(path)
                    path.write_text("".join(json.dumps(event) + "\n" for event in changed))
                with self.assertRaisesRegex(RuntimeError, diagnostic):
                    verify_traces(paths, 100, self.summary["limits"], self.manifest)

    def test_resource_overshoot_cannot_pass(self):
        for field in (
            "max_providers",
            "max_loaded",
            "sessions_per_codex_process",
            "max_active_turns",
        ):
            with self.subTest(limit=field):
                with self.assertRaisesRegex(RuntimeError, "Resource limit exceeded"):
                    # The eviction fixture now deliberately uses one process;
                    # set its bound below the observed peak to prove rejection.
                    bound = (
                        self.summary["peak_mock_processes"] - 1 if field == "max_providers" else 1
                    )
                    verify_traces(
                        self.traces, 100, {**self.summary["limits"], field: bound}, self.manifest
                    )

    def test_existing_evidence_directory_is_not_modified(self):
        before = {p.name: p.read_bytes() for p in self.output.iterdir() if p.is_file()}
        result = run_process(
            [
                sys.executable,
                str(ROOT / "run.py"),
                "mock",
                "all-to-one",
                "--agents",
                "100",
                "--output-dir",
                str(self.output),
            ],
            capture_output=True,
            text=True,
            timeout=10,
        )
        self.assertNotEqual(result.returncode, 0)
        after = {p.name: p.read_bytes() for p in self.output.iterdir() if p.is_file()}
        self.assertEqual(before, after)


class HighResidencyMockTests(MockScenarioTestCase):
    preset = None  # Exercise the CLI default; eviction remains explicitly selected.

    def test_hundred_agents_exchange_without_forced_recycling(self):
        self.assertEqual(self.summary["result"], "passed")
        self.assertEqual(self.summary["preset"], "high-residency")
        self.assertFalse(self.summary["recycling_required"])
        self.assertEqual(self.summary["limits"]["max_loaded"], 100)
        self.assertEqual(self.summary["limits"]["max_providers"], 16)
        self.assertEqual(self.summary["messages"], 397)
        self.assertEqual(self.summary["native_conversations"], 100)
        self.assertEqual(self.summary["explicit_cancellations"], 99)
        self.assertGreaterEqual(self.summary["peak_same_process_turns"], 2)
        # Relaxing the recycling requirement must not relax delivery/identity
        # checks. Exercise the same verifier mode against corrupted evidence.
        path = Path(self.temporary.name) / "bad-high-residency.sqlite"
        shutil.copyfile(self.output / "runtime.sqlite", path)
        with closing(sqlite3.connect(path)) as db:
            db.execute("UPDATE messages SET state='accepted' WHERE body='received pong'")
            db.commit()
        with self.assertRaisesRegex(RuntimeError, "Invalid delivery lifecycle"):
            verify_database(path, self.root, 100, require_recycling=False)


class MockLimitTests(unittest.TestCase):
    def test_resume_is_required_only_when_eviction_forces_it(self):
        for flags, expected in (
            ([], False),
            (["--preset", "eviction"], False),
            (["--preset", "eviction", "--max-providers", "1"], True),
        ):
            with self.subTest(flags=flags):
                scenario = object.__new__(AllToOneRun)
                scenario.args = parse_args(flags)
                scenario.limits = runtime_limits(scenario.args)
                self.assertEqual(scenario.recycling_required, expected)

    def test_high_residency_can_cover_whole_graph_and_scale_processes(self):
        limits = runtime_limits(parse_args(["--agents", "1000"]))
        self.assertEqual(limits["max_loaded"], 1000)
        self.assertEqual(limits["max_active_turns"], 256)
        self.assertEqual(limits["max_starting"], 32)
        limits = runtime_limits(
            parse_args(
                [
                    "--agents",
                    "100000",
                    "--preset",
                    "high-residency",
                    "--max-loaded",
                    "2048",
                    "--max-providers",
                    "32",
                    "--max-active-turns",
                    "128",
                    "--max-starting",
                    "24",
                ]
            )
        )
        self.assertEqual(limits["sessions_per_codex_process"], 64)
        self.assertEqual(limits["max_providers"], 32)
        self.assertEqual(limits["max_active_turns"], 128)
        self.assertEqual(limits["max_starting"], 24)

    def test_invalid_limits_are_rejected_before_creating_evidence(self):
        cases = [
            (["--max-providers", "0"], "--max-providers"),
            (["--max-loaded", "7"], "--max-loaded"),
            (["--max-starting", "0"], "--max-starting"),
            (["--max-active-turns", "1"], "--max-active-turns"),
            (["--warm-providers", "-1"], "--warm-providers"),
            (["--warm-providers", "17"], "--warm-providers"),
            (["--preset", "eviction", "--warm-providers", "5"], "--warm-providers"),
            (
                ["--agents", "100000", "--max-loaded", "5000", "--max-providers", "4"],
                "sessions_per_codex_process",
            ),
            (["--max-loaded", "64", "--max-providers", "64"], "sessions_per_codex_process"),
        ]
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "must-not-exist"
            for flags, diagnostic in cases:
                with self.subTest(flags=flags):
                    error = io.StringIO()
                    with redirect_stderr(error), self.assertRaises(SystemExit) as failure:
                        parse_args(flags + ["--output-dir", str(output)])
                    self.assertEqual(failure.exception.code, 2)
                    self.assertIn(diagnostic, error.getvalue())
                    self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
