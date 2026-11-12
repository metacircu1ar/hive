"""Reject missing, changed and misordered provider admission evidence."""

import copy
import json
from pathlib import Path
import sqlite3
import sys
import unittest

from e2e.scripts.verify_ping_pong_pair import PairDatabaseAudit, SCHEMA


class ProviderAuditTests(unittest.TestCase):
    def setUp(self):
        self.events = [
            ("started", {"pid": 123}),
            (
                "initialized",
                {
                    "compatibility": {
                        "registry": {
                            "corpus_versions": ["0.154.0", "0.161.0"],
                            "last_tested": "0.161.0",
                            "minimum": "0.154.0",
                            "provider": "codex",
                        },
                        "support": "untested",
                        "version": "0.163.0",
                    },
                    "reply": {},
                },
            ),
            ("shutdown", {"shutdown": {"confirmed": True, "quiescent": True, "exit_code": 0}}),
        ]

    def verify(self, events, *, reference=False):
        db, audit = sqlite3.connect(":memory:"), sqlite3.connect(":memory:")
        try:
            db.row_factory = sqlite3.Row
            db.execute(
                "CREATE TABLE provider_events(seq INTEGER PRIMARY KEY,provider TEXT,kind TEXT,data TEXT)"
            )
            db.executemany(
                "INSERT INTO provider_events VALUES(?,?,?,?)",
                [
                    (seq, "provider", kind, json.dumps(data))
                    for seq, (kind, data) in enumerate(events)
                ],
            )
            audit.executescript(SCHEMA)
            verifier = PairDatabaseAudit(db, audit, None, 0)
            verifier.expect_initialization = not reference
            verifier.verify_providers()
            self.assertEqual(verifier.providers, 1)
        finally:
            db.close()
            audit.close()

    def test_current_admission(self):
        self.verify(self.events)

    def test_missing_duplicate_and_late_admission(self):
        for events in (
            self.events[::2],
            self.events[:2] + self.events[1:],
            [self.events[1], self.events[0], self.events[2]],
            [self.events[0], self.events[2], self.events[1]],
        ):
            with self.subTest(events=events), self.assertRaises(RuntimeError):
                self.verify(events)

    def test_wrong_and_missing_version_observation(self):
        for field in ("registry", "version", "support"):
            events = copy.deepcopy(self.events)
            events[1][1]["compatibility"].pop(field)
            with self.subTest(field=field), self.assertRaisesRegex(RuntimeError, "compatibility"):
                self.verify(events)
        events = copy.deepcopy(self.events)
        events[1][1]["compatibility"]["version"] = "0.154.0"
        with self.assertRaisesRegex(RuntimeError, "compatibility"):
            self.verify(events)

    def test_unknown_events_are_rejected(self):
        with self.assertRaisesRegex(RuntimeError, "Unexpected provider event"):
            self.verify([*self.events, ("unknown", {})])

    def test_reference_contract_remains_exact(self):
        self.verify(self.events[::2], reference=True)
        with self.assertRaisesRegex(RuntimeError, "Unexpected provider event"):
            self.verify(self.events, reference=True)


if __name__ == "__main__":
    unittest.main()
