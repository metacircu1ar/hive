"""Mock pair runs and independent current-evidence corruption checks."""

from e2e.scripts.harness.scenario import run as run_process
import json
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from contextlib import closing
from pathlib import Path
from e2e.scripts.verify_ping_pong_pair import json_lines, verify

ROOT = Path(__file__).resolve().parents[1]


DATABASE_CORRUPTIONS = [
    (
        "UPDATE events SET execution=NULL WHERE kind IN ('provider_started','session_ready','turn_started','turn_finished')",
        "Missing execution identity",
    ),
    (
        "UPDATE events SET execution=(SELECT e.execution FROM events e JOIN agents a ON a.id=e.agent WHERE a.parent_id=events.agent AND e.kind='provider_started' LIMIT 1) WHERE agent=(SELECT id FROM agents WHERE parent_id IS NULL LIMIT 1) AND kind IN ('turn_started','turn_finished')",
        "Turn uses another agent execution",
    ),
    (
        "INSERT INTO events(agent,execution,at,kind,data) VALUES('unknown','unknown',0,'tool_completed','{}')",
        "Orphan event",
    ),
    (
        "UPDATE events SET seq=-seq WHERE kind='task_completed'",
        "Invalid pair completion order",
    ),
    (
        "UPDATE agents SET parent_id=(SELECT id FROM agents WHERE parent_id IS NULL LIMIT 1) WHERE parent_id IS NOT NULL",
        "Expected one pair child",
    ),
    (
        "UPDATE messages SET sender=(SELECT id FROM agents WHERE parent_id IS NULL LIMIT 1) WHERE body='pong'",
        "Cross-pair message",
    ),
    (
        "UPDATE events SET data=json_set(data,'$.by','human') WHERE kind='termination_requested'",
        "Cross-pair termination",
    ),
    (
        "UPDATE events SET data=json_set(data,'$.sender_turn_id','wrong') WHERE kind='message_accepted'",
        "Wrong sending-turn provenance",
    ),
    (
        "DELETE FROM events WHERE seq=(SELECT max(seq) FROM events WHERE kind='turn_finished')",
        "Wrong turn_finished count",
    ),
    (
        "INSERT INTO events(agent,execution,at,kind,data) SELECT agent,execution,at,kind,data FROM events WHERE kind='session_ready' LIMIT 1",
        "Wrong session execution binding",
    ),
    (
        "UPDATE agents SET control='stop' WHERE parent_id IS NULL",
        "Agent did not stop cleanly",
    ),
    (
        "UPDATE events SET data=json_set(data,'$.effort','high') WHERE kind='turn_started'",
        "Invalid turn lifecycle",
    ),
    (
        "UPDATE provider_events SET data=json_set(data,'$.shutdown.confirmed',json('false')) WHERE kind='shutdown'",
        "Unconfirmed provider cleanup",
    ),
    (
        "UPDATE provider_events SET data=json_set(data,'$.shutdown.exit_code',1) WHERE kind='shutdown'",
        "Unconfirmed provider cleanup",
    ),
    (
        "UPDATE provider_events SET data=json_set(data,'$.error_code','oops') WHERE kind='shutdown'",
        "Unconfirmed provider cleanup",
    ),
    (
        "UPDATE messages SET state='accepted' WHERE body='pong'",
        "Invalid turn lifecycle",
    ),
    (
        "DELETE FROM agents WHERE id=(SELECT id FROM agents WHERE parent_id IS NULL LIMIT 1) OR parent_id=(SELECT id FROM agents WHERE parent_id IS NULL LIMIT 1)",
        "Wrong agent count",
    ),
    (
        "DELETE FROM events WHERE kind='termination_requested'",
        "Wrong termination_requested count",
    ),
    (
        "INSERT INTO events(agent,execution,at,kind,data) SELECT agent,execution,at,'provider_error','{}' FROM events LIMIT 1",
        "Unexpected runtime event kind",
    ),
    (
        "UPDATE provider_events SET kind='provider_error' WHERE kind='started'",
        "Unexpected provider event kind",
    ),
]


class PairTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix="hl-pair-test-")
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.output = Path(cls.temporary.name) / "run"
        result = run_process(
            [
                sys.executable,
                str(ROOT / "run.py"),
                "mock",
                "pairs",
                "--agents",
                "100",
                "--debug-timings",
                "--observer-interval",
                "0.05",
                "--output-dir",
                str(cls.output),
                "--timeout",
                "120",
            ],
            capture_output=True,
            text=True,
            timeout=240,
        )
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)
        cls.result = json.loads((cls.output / "verification.json").read_text())
        cls.database = cls.output / "state/runtime/runtime.sqlite"
        cls.traces = list(
            (cls.output / "state").glob("profiles/*/provider/scale-mock/trace-*.jsonl")
        )

    def check(self, database=None, traces=None, limits=None, submissions=None):
        with tempfile.TemporaryDirectory(dir=self.temporary.name) as scratch:
            return verify(
                database or self.database,
                submissions or self.output / "submissions.jsonl",
                traces or self.traces,
                100,
                limits or self.result["limits"],
                Path(scratch) / "audit.sqlite",
            )

    def test_rejects_missing_runtime_counts_table(self):
        with tempfile.TemporaryDirectory(dir=self.temporary.name) as directory:
            database = Path(directory) / "runtime.sqlite"
            shutil.copy2(self.database, database)
            with closing(sqlite3.connect(database)) as db:
                db.execute("DROP TABLE runtime_counts")
            with self.assertRaisesRegex(sqlite3.OperationalError, "no such table: runtime_counts"):
                self.check(database=database)

    def test_auth_and_catalog_traces_contain_only_metadata(self):
        metadata = 0
        for path in self.traces:
            rows = list(json_lines(path))
            if not rows[1]["runtime"]:
                metadata += 1
                self.assertEqual(
                    [r["kind"] for r in rows], ["process_start", "connection", "process_exit"]
                )
        self.assertEqual(metadata, 2)  # Auth bootstrap and one cold catalog lookup.

    def test_exact_pairs_and_cleanup(self):
        for name, value in dict(
            agents=100,
            pairs=50,
            messages=200,
            turns=200,
            pings=50,
            pongs=50,
            explicit_cancellations=50,
            self_completions=50,
            native_conversations=100,
        ).items():
            self.assertEqual(self.result[name], value, name)
        self.assertEqual(self.result["result"], "passed")
        self.assertGreaterEqual(self.result["peak_same_process_turns"], 2)

    def test_streaming_observers_record_complete_success_and_separate_cpu(self):
        accounting = json.loads((self.output / "observer-accounting.json").read_text())
        self.assertEqual(accounting["errors"], [])
        fast = json.loads((self.output / "fast-control-summary.json").read_text())
        self.assertEqual(
            fast["socket"]["seconds"]["count"], accounting["threads"]["control"]["samples"]
        )
        for kind in ("control", "progress"):
            self.assertGreater(accounting["threads"][kind]["samples"], 0)
            self.assertGreater(accounting["threads"][kind]["cpu_ns"], 0)

    def test_corrupted_mailbox_counter_is_detected_independently(self):
        with tempfile.TemporaryDirectory(dir=self.temporary.name) as directory:
            database = Path(directory) / "runtime.sqlite"
            shutil.copy2(self.database, database)
            with closing(sqlite3.connect(database)) as db:
                db.execute(
                    "UPDATE agents SET pending_count=1 WHERE rowid=(SELECT min(rowid) FROM agents)"
                )
                db.commit()
            with self.assertRaisesRegex(RuntimeError, "Mailbox counters disagree"):
                self.check(database=database)

    def test_corrupted_head_is_detected_independently(self):
        with tempfile.TemporaryDirectory(dir=self.temporary.name) as directory:
            database = Path(directory) / "runtime.sqlite"
            shutil.copy2(self.database, database)
            with closing(sqlite3.connect(database)) as db:
                # A well-formed pair of cached fields can still refer to the wrong
                # message. The verifier must recompute from accepted messages.
                db.execute(
                    "UPDATE agents SET pending_seq=1,head_startup=0 WHERE rowid=(SELECT min(rowid) FROM agents)"
                )
                db.commit()
            with self.assertRaisesRegex(RuntimeError, "Mailbox counters disagree"):
                self.check(database=database)

    def test_current_debug_timings_and_no_legacy_sidecars(self):
        summary = json.loads((self.output / "debug-timings-summary.json").read_text())
        self.assertFalse(summary["exhaustive"])
        self.assertTrue(summary["observed_start"])
        self.assertTrue(summary["observed_clean_stop"])
        for event in ["writer.batch", "state.sync", "runtime.timing"]:
            self.assertGreater(summary["events"][event], 0)
        runtime = self.output / "state/runtime"
        self.assertFalse(list(runtime.glob("profile-*.jsonl")))
        self.assertFalse(list(runtime.glob("diagnostics-*.jsonl")))
        for row in json_lines(self.output / "control.jsonl"):
            self.assertNotIn("diagnostic_control", row["status"])

    def test_progress_counter_brackets_precede_matching_control_probe(self):
        progress = list(json_lines(self.output / "progress.jsonl"))
        control = list(json_lines(self.output / "control.jsonl"))
        self.assertEqual(len(progress), len(control))
        for row, probe in zip(progress, control):
            self.assertLessEqual(
                row["event_snapshot_before_monotonic_ns"], row["event_snapshot_after_monotonic_ns"]
            )
            self.assertLessEqual(
                row["event_snapshot_after_monotonic_ns"], probe["start_monotonic_ns"]
            )
            self.assertLessEqual(probe["end_monotonic_ns"], row["monotonic_ns"])

    def test_corrupted_database_cannot_pass(self):
        mutations = DATABASE_CORRUPTIONS
        # The unmodified copy must pass first, proving the snapshot is complete.
        control = Path(self.temporary.name) / "clean.sqlite"
        shutil.copyfile(self.database, control)
        self.check(database=control)
        for i, (sql, diagnostic) in enumerate(mutations):
            with self.subTest(sql=sql):
                path = Path(self.temporary.name) / f"bad-{i}.sqlite"
                shutil.copyfile(self.database, path)
                with closing(sqlite3.connect(path)) as db:
                    self.assertGreater(db.execute(sql).rowcount, 0)
                    db.commit()
                with self.assertRaisesRegex((RuntimeError, sqlite3.IntegrityError), diagnostic):
                    self.check(database=path)

    def test_interrupted_ping_without_cancellation_order_fails(self):
        path = Path(self.temporary.name) / "bad-interrupt.sqlite"
        shutil.copyfile(self.database, path)
        with closing(sqlite3.connect(path)) as db:
            mid = db.execute("SELECT id FROM messages WHERE body='ping' LIMIT 1").fetchone()[0]
            # Make room for a deterministic bad finish before cancellation,
            # independent of which side won the real fixture's cancellation race.
            db.execute("UPDATE events SET seq=-seq*2")
            db.execute("UPDATE events SET seq=-seq")
            cancelled = db.execute(
                "SELECT e.seq FROM events e JOIN messages m ON m.recipient=e.agent WHERE m.id=? AND e.kind='termination_requested'",
                (mid,),
            ).fetchone()[0]
            self.assertEqual(
                db.execute(
                    "UPDATE events SET seq=? WHERE kind='turn_finished' AND json_extract(data,'$.message_id')=?",
                    (cancelled - 1, mid),
                ).rowcount,
                1,
            )
            self.assertEqual(
                db.execute("UPDATE messages SET state='interrupted' WHERE id=?", (mid,)).rowcount,
                1,
            )
            self.assertEqual(
                db.execute(
                    "UPDATE events SET data=json_set(data,'$.status','interrupted') WHERE kind='turn_finished' AND json_extract(data,'$.message_id')=?",
                    (mid,),
                ).rowcount,
                1,
            )
            db.commit()
        with self.assertRaisesRegex(
            RuntimeError, "Interrupted ping without reply and cancellation"
        ):
            self.check(database=path)

    def test_clean_pre_session_retirement_is_allowed_but_not_native_work(self):
        path = Path(self.temporary.name) / "pre-session.sqlite"
        shutil.copyfile(self.database, path)
        with closing(sqlite3.connect(path)) as db:
            db.execute("UPDATE events SET seq=-seq*10")
            db.execute("UPDATE events SET seq=-seq")
            seq, agent, at, data = db.execute(
                "SELECT seq,agent,at,data FROM events WHERE kind='provider_started' ORDER BY seq LIMIT 1"
            ).fetchone()
            for offset, kind, payload in [
                (5, "execution_starting", "{}"),
                (4, "provider_started", data),
                (
                    3,
                    "provider_session_released",
                    json.dumps(dict(confirmed=True, quiescent=True)),
                ),
                (2, "execution_stopped", json.dumps(dict(task_outcome="open"))),
            ]:
                db.execute(
                    "INSERT INTO events(seq,agent,execution,at,kind,data) VALUES(?,?,?,?,?,?)",
                    (seq - offset, agent, "unstarted-execution", at, kind, payload),
                )
            db.commit()
        self.assertEqual(self.check(database=path)["pre_session_releases"], 1)
        for name, sql, diagnostic in [
            (
                "unproven",
                "UPDATE events SET data=json_set(data,'$.confirmed',json('false')) WHERE execution='unstarted-execution' AND kind='provider_session_released'",
                "Unproven pre-session release",
            ),
            (
                "missing-stop",
                "DELETE FROM events WHERE execution='unstarted-execution' AND kind='execution_stopped'",
                "Expected one pre-session stop",
            ),
            (
                "native-work",
                "UPDATE events SET kind='thread_bound' WHERE execution='unstarted-execution' AND kind='execution_starting'",
                "Pre-session release contains native work",
            ),
        ]:
            with self.subTest(case=name):
                bad = Path(self.temporary.name) / f"pre-session-{name}.sqlite"
                shutil.copyfile(path, bad)
                with closing(sqlite3.connect(bad)) as db:
                    self.assertGreater(db.execute(sql).rowcount, 0)
                    db.commit()
                with self.assertRaisesRegex(RuntimeError, diagnostic):
                    self.check(database=bad)

    def test_corrupted_traces_cannot_pass(self):
        for change, diagnostic in [
            ("agent", "Wrong trace conversation"),
            ("message", "Wrong trace turn identity"),
            ("status", "Wrong trace turn status"),
            ("missing_exit", "Incomplete trace lifecycle"),
            ("no_overlap", "No verified overlap"),
            ("pid", "Provider process identity mismatch"),
            ("metadata_as_runtime", "Provider process identity mismatch"),
            ("barrier", "False overlap barrier"),
            ("thread", "Overlapping/nonresident turn"),
        ]:
            with (
                self.subTest(change=change),
                tempfile.TemporaryDirectory(dir=self.temporary.name) as directory,
            ):
                paths = []
                for p in self.traces:
                    path = Path(directory) / p.name
                    paths.append(path)
                    with path.open("w") as out:
                        for e in json_lines(p):
                            if change == "metadata_as_runtime" and e["kind"] == "connection":
                                e["runtime"] = True
                            if change == "pid":
                                e["pid"] += 1000000
                            if change == "barrier" and e["kind"] == "barrier":
                                e["threads"] = ["fake-one", "fake-two"]
                                e["overlapped"] = True
                            if change == "thread" and e["kind"] == "turn_start":
                                e["thread"] = "wrong"
                            if change == "agent" and e["kind"] == "thread_start":
                                e["agent"] = "wrong"
                            if change == "message" and e["kind"] == "turn_start":
                                e["message"] = "wrong"
                            if change == "status" and e["kind"] == "turn_end":
                                e["status"] = "failed"
                            if change == "missing_exit" and e["kind"] == "process_exit":
                                continue
                            if change == "no_overlap" and e["kind"] == "barrier":
                                e["overlapped"] = False
                            out.write(json.dumps(e) + "\n")
                with self.assertRaisesRegex((RuntimeError, sqlite3.IntegrityError), diagnostic):
                    self.check(traces=paths)

    def test_capacity_overshoots_fail(self):
        for key in (
            "max_loaded",
            "max_providers",
            "max_active_turns",
            "sessions_per_codex_process",
        ):
            with (
                self.subTest(limit=key),
                self.assertRaisesRegex(RuntimeError, "Resource limit exceeded"),
            ):
                self.check(limits={**self.result["limits"], key: 1})

    def test_wrong_submitted_root_fails(self):
        path = Path(self.temporary.name) / "bad-seeds.jsonl"
        entries = list(json_lines(self.output / "submissions.jsonl"))
        entries[0]["agent_id"] = "unsubmitted"
        path.write_text("".join(json.dumps(e) + "\n" for e in entries))
        with self.assertRaisesRegex(RuntimeError, "Unsubmitted root"):
            self.check(submissions=path)

    def test_existing_evidence_is_not_overwritten(self):
        before = (self.output / "verification.json").read_bytes()
        result = run_process(
            [
                sys.executable,
                str(ROOT / "run.py"),
                "mock",
                "pairs",
                "--output-dir",
                str(self.output),
            ],
            capture_output=True,
            timeout=10,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.output / "verification.json").read_bytes(), before)


class PairResumeTests(unittest.TestCase):
    def test_recycling_keeps_pair_identity_and_conversation(self):
        with tempfile.TemporaryDirectory(prefix="hl-pair-resume-") as directory:
            output = Path(directory) / "run"
            result = run_process(
                [
                    sys.executable,
                    str(ROOT / "run.py"),
                    "mock",
                    "pairs",
                    "--agents",
                    "100",
                    "--debug-timings",
                    "--idle-seconds",
                    "1",
                    "--max-loaded",
                    "8",
                    "--max-providers",
                    "2",
                    "--max-active-turns",
                    "4",
                    "--output-dir",
                    str(output),
                    "--timeout",
                    "120",
                ],
                capture_output=True,
                text=True,
                timeout=240,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            summary = json.loads((output / "verification.json").read_text())
            records = list(json_lines(output / "debug-timings.jsonl"))
            decisions = [r for r in records if r["event"] == "provider.capacity_drain"]
            self.assertGreater(len(decisions), 0)
            self.assertEqual(summary["limits"]["idle_seconds"], 1)
            self.assertGreater(summary["resumed_agents"], 0)
            self.assertLessEqual(summary["peak_resident_threads"], 8)


if __name__ == "__main__":
    unittest.main()
