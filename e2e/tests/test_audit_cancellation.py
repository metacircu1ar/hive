"""Cancel actual record ingestion, SQLite work and synchronous storage observation."""

import json
from pathlib import Path
import signal
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

from e2e.scripts.harness.audit import cancellable
from e2e.scripts.harness.traces import ingest_trace
from e2e.scripts import run_arms
from release.cancellation import Cancelled, Scope


class AuditCancellationTests(unittest.TestCase):
    def test_cancel_actual_trace_ingestion_preserves_original_complete_evidence(self):
        with tempfile.TemporaryDirectory() as temp, Scope() as state:
            path = Path(temp) / "trace"
            common = dict(process="process", pid=1)
            rows = [
                dict(common, kind="process_start", ns=1),
                dict(common, kind="connection", ns=2, runtime=True),
            ]
            rows += [dict(common, kind="frame", ns=index) for index in range(3, 10003)]
            rows.append(dict(common, kind="process_exit", ns=10003))
            original = "".join(json.dumps(row) + "\n" for row in rows).encode()
            path.write_bytes(original)

            class Evidence:
                frames = 0

                def frame(self, row, raw):
                    self.frames += 1
                    if self.frames == 10:
                        state.record(signal.SIGTERM, None)

            evidence = Evidence()
            with self.assertRaises(Cancelled) as raised:
                ingest_trace(path, evidence)
            self.assertEqual(raised.exception.number, signal.SIGTERM)
            self.assertEqual(evidence.frames, 10)
            self.assertEqual(path.read_bytes(), original)

    def test_native_sqlite_statement_is_interrupted_with_actual_cancellation(self):
        with sqlite3.connect(":memory:") as connection, Scope() as state:
            connection.execute("CREATE TABLE original(value)")
            connection.execute("INSERT INTO original VALUES(7)")

            def cancel():
                state.record(signal.SIGTERM, None)
                return 1

            connection.create_function("cancel_here", 0, cancel)
            with self.assertRaises(Cancelled), cancellable(connection):
                connection.execute("""
                    WITH RECURSIVE many(n) AS (
                        SELECT cancel_here() UNION ALL SELECT n+1 FROM many WHERE n<100000000
                    ) SELECT sum(n) FROM many
                """).fetchone()
            self.assertEqual(connection.execute("SELECT value FROM original").fetchone(), (7,))

    def test_canary_honors_pending_signal_and_removes_private_probe_file(self):
        with tempfile.TemporaryDirectory() as temp, Scope() as state:
            with patch.object(
                run_arms, "sync", side_effect=lambda *_: state.record(signal.SIGTERM, None)
            ):
                with self.assertRaises(Cancelled):
                    run_arms.canary(Path(temp))
            self.assertEqual(list(Path(temp).iterdir()), [])
