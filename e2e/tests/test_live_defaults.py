"""Check live scenario defaults without starting a daemon or provider."""

import argparse
import importlib
import re
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
from e2e.scripts.product_constants import (
    LIVE_CODEX_CHANGED_EFFORT,
    LIVE_CODEX_EFFORT,
    LIVE_CODEX_MODEL,
    LIVE_REASONING_EFFORT,
)


class ParserReady(Exception):
    def __init__(self, parser):
        self.parser = parser


def defaults(module):
    def stop(parser):
        raise ParserReady(parser)

    with patch.object(argparse.ArgumentParser, "parse_args", autospec=True, side_effect=stop):
        try:
            importlib.import_module("e2e.scripts." + module).main()
        except ParserReady as ready:
            return ready.parser
    raise AssertionError("Scenario did not expose an argument parser")


class LiveDefaults(unittest.TestCase):
    def test_live_harnesses_and_example_agree(self):
        for module in ("run_factorial_demo",):
            with self.subTest(module=module):
                parser = defaults(module)
                self.assertEqual(parser.get_default("expected_model"), "gpt-6.1-terra")
                self.assertEqual(parser.get_default("expected_effort"), LIVE_REASONING_EFFORT)
        parser = defaults("run_conversation_controls")
        self.assertEqual(parser.get_default("codex_model"), LIVE_CODEX_MODEL)
        self.assertEqual(parser.get_default("codex_changed_model"), "gpt-6-sol")
        self.assertEqual(parser.get_default("codex_changed_effort"), LIVE_CODEX_CHANGED_EFFORT)
        self.assertEqual(parser.get_default("claude_changed_model"), "haiku")
        self.assertEqual(parser.get_default("claude_changed_effort"), "low")
        self.assertEqual((LIVE_CODEX_EFFORT, LIVE_CODEX_CHANGED_EFFORT), ("low", "medium"))
        config = (ROOT / "examples/runtime.yaml").read_text()
        self.assertEqual(
            re.findall(r"^    model: (\S+)$", config, re.MULTILINE),
            ["gpt-6.1-terra"] * 3 + [LIVE_CODEX_MODEL],
        )
        self.assertEqual(
            re.findall(r"^    effort: (\S+)$", config, re.MULTILINE),
            [LIVE_REASONING_EFFORT] * 3 + [LIVE_CODEX_EFFORT],
        )
