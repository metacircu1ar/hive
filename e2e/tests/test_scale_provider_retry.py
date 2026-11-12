"""A mock agent honors explicit admission backpressure without duplicating work."""

import collections
import copy
import importlib.util
import json
from pathlib import Path
import sys
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import patch


def fixture_module():
    common = ModuleType("mock_common")
    for name in (
        "schema_probe",
        "decode_scenario",
        "publish_json",
        "append_jsonl",
        "CompleteLines",
        "AppendStream",
    ):
        setattr(common, name, None)
    path = Path(__file__).parent / "fixtures/scale_provider.py"
    spec = importlib.util.spec_from_file_location("scale_retry_fixture", path)
    module = importlib.util.module_from_spec(spec)
    with patch.dict(sys.modules, {"mock_common": common}):
        spec.loader.exec_module(module)
    return module


class SpawnRetryTests(unittest.TestCase):
    def setUp(self):
        self.module = fixture_module()
        self.now = 100.0
        self.module.time = SimpleNamespace(monotonic=lambda: self.now)
        self.provider = self.module.Provider.__new__(self.module.Provider)
        self.frames, self.journal = [], []
        self.provider.emit = self.frames.append
        self.provider.log = lambda *_args, **_kwargs: None
        self.provider.journal = lambda thread, change: self.journal.append((thread, change))
        self.provider.pending = {}
        self.provider.turns, self.provider.threads = {}, {}
        self.provider.barrier_deadline = None
        self.provider.deltas = 0
        self.arguments = {"role": "pair-pong", "body": "Wait for ping.", "request_id": "spawn-pong"}
        self.add_turn("thread", "turn", "hive_spawn", self.arguments)
        self.provider.advance("thread")

    def add_turn(self, thread, turn, tool, arguments):
        self.provider.threads[thread] = {"tools": [{"name": tool}, {"name": "hive_send"}]}
        self.provider.turns[thread] = dict(
            id=turn, action=0, actions=collections.deque([(tool, arguments)])
        )

    def calls(self):
        return [frame for frame in self.frames if frame.get("method") == "item/tool/call"]

    def reply(self, value=None, *, call=None, success=False):
        self.provider.handle_tool_result(
            {
                "id": (call or self.calls()[-1])["id"],
                "result": {
                    "success": success,
                    "contentItems": [
                        {
                            "type": "inputText",
                            "text": json.dumps(
                                value if value is not None else self.module.SPAWN_RETRY_ERROR
                            ),
                        }
                    ],
                },
            }
        )

    def tick(self):
        self.now = self.provider.turns["thread"]["retry_at"]
        self.provider.advance_retries()

    def test_retry_keeps_request_identity_and_creates_one_child(self):
        expected = copy.deepcopy(self.arguments)
        for _ in range(2):
            self.reply()
            self.assertEqual(self.journal, [])
            completed = self.frames[-1]["params"]["item"]
            self.assertIs(completed["success"], False)
            self.assertEqual(completed["arguments"], expected)
            self.assertGreater(self.provider.next_timeout(), 0)
            self.tick()
        calls = self.calls()
        self.assertEqual(len(calls), 3)
        self.assertEqual(len({call["id"] for call in calls}), 3)
        self.assertEqual([call["params"]["arguments"] for call in calls], [expected] * 3)
        self.reply({"agent_id": "child"}, success=True)
        self.assertEqual(self.journal, [("thread", {"children": ["child"]})])
        self.assertNotIn("spawn_retries", self.provider.turns["thread"])
        self.assertEqual(self.calls()[-1]["params"]["tool"], "hive_send")
        self.assertEqual(self.calls()[-1]["params"]["arguments"]["target"], "child")

    def test_other_conversation_progresses_without_early_retry(self):
        self.reply()
        retry_at = self.provider.turns["thread"]["retry_at"]
        self.add_turn("other", "other-turn", "hive_send", {"target": "peer", "body": "ping"})
        self.provider.advance("other")
        self.assertEqual(self.calls()[-1]["params"]["threadId"], "other")
        self.now = retry_at - 0.001
        self.provider.advance_retries()
        self.assertEqual(len(self.calls()), 2)
        self.assertGreater(self.provider.next_timeout(), 0)
        self.tick()
        self.assertEqual(self.calls()[-1]["params"]["threadId"], "thread")

    def test_backoff_is_bounded_and_exhaustion_still_fails(self):
        delays = []
        for _ in range(self.module.SPAWN_RETRY_LIMIT):
            self.reply()
            delays.append(self.provider.next_timeout())
            self.tick()
        self.assertEqual(delays, sorted(delays))
        self.assertLessEqual(max(delays), 1)
        self.assertLessEqual(sum(delays), 16)
        with self.assertRaisesRegex(RuntimeError, "retry limit exceeded"):
            self.reply()
        self.assertEqual(len(self.calls()), self.module.SPAWN_RETRY_LIMIT + 1)
        self.assertEqual(self.journal, [])

    def interrupt(self):
        self.provider.handle(
            {
                "id": "interrupt",
                "method": "turn/interrupt",
                "params": {"threadId": "thread", "turnId": "turn"},
            }
        )

    def test_interrupt_discards_scheduled_retry(self):
        self.reply()
        self.now = self.provider.turns["thread"]["retry_at"] + 1
        self.interrupt()
        self.provider.advance_retries()
        self.assertIsNone(self.provider.next_timeout())
        self.assertEqual(len(self.calls()), 1)
        self.assertEqual(self.journal, [])

    def test_late_reply_cannot_schedule_work_in_a_new_turn(self):
        self.reply()
        self.tick()
        old = self.calls()[-1]
        self.interrupt()
        self.add_turn("thread", "new-turn", "hive_send", {"target": "peer", "body": "new"})
        self.reply(call=old)
        self.provider.advance_retries()
        self.assertNotIn("retry_at", self.provider.turns["thread"])
        self.assertEqual(len(self.calls()), 2)
        self.provider.advance("thread")
        self.assertEqual(self.calls()[-1]["params"]["turnId"], "new-turn")
        self.assertEqual(self.calls()[-1]["params"]["tool"], "hive_send")

    def test_other_errors_are_not_retried(self):
        for index, error in enumerate(
            (
                {"code": "provider_unavailable", "message": "uncertain input"},
                {
                    "code": "agent_busy",
                    "message": "No safe provider slot is available for model discovery",
                },
                {"code": "agent_busy", "message": "Agent is busy"},
            )
        ):
            with self.subTest(error=error):
                thread = f"error-{index}"
                self.add_turn(thread, f"turn-{index}", "hive_spawn", self.arguments)
                self.provider.advance(thread)
                with self.assertRaisesRegex(RuntimeError, "Tool failed"):
                    self.reply({"error": error})
                self.assertNotIn("retry_at", self.provider.turns[thread])

    def test_malformed_content_and_other_tools_do_not_match(self):
        for result in (
            {"success": False},
            {"success": False, "contentItems": []},
            {"success": False, "contentItems": [{"type": "inputText", "text": 3}]},
        ):
            self.assertFalse(self.module.catalog_spawn_busy("hive_spawn", result))
        result = {
            "success": False,
            "contentItems": [
                {"type": "inputText", "text": json.dumps(self.module.SPAWN_RETRY_ERROR)}
            ],
        }
        self.assertFalse(self.module.catalog_spawn_busy("hive_send", result))
        result["contentItems"][0]["text"] = "{"
        with self.assertRaises(json.JSONDecodeError):
            self.module.catalog_spawn_busy("hive_spawn", result)

    def test_barrier_and_retry_share_the_earliest_deadline(self):
        self.reply()
        self.provider.barrier_deadline = self.now + 0.01
        self.assertAlmostEqual(self.provider.next_timeout(), 0.01)


if __name__ == "__main__":
    unittest.main()
