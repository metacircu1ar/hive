"""Command families select explicit prefixes; payload arguments remain untouched."""

import sys
import subprocess
import unittest
from pathlib import Path
from unittest.mock import patch

from e2e.scripts.harness.cli import Cli


class CommandPrefixes(unittest.TestCase):
    def test_remote_control_and_provider_prefixes(self):
        remote = ["hivectl", "--state-dir", "/state"]
        configured = remote + ["--config", "/config"]
        call = Cli(
            remote, prefixes={"daemon": configured, "auth": configured, "models": configured}
        )
        for args in [
            ("spawn", "--body", "--config"),
            ("send", "agent", "--body", "hello"),
            ("status", "agent"),
            ("agents",),
        ]:
            self.assertEqual(call.argv(*args), remote + list(args))
        for args in [
            ("daemon", "status"),
            ("daemon", "stop"),
            ("auth", "status", "main"),
            ("models", "--auth", "main"),
        ]:
            self.assertEqual(call.argv(*args), configured + list(args))
        self.assertEqual(call.command, remote)
        self.assertNotIn("--config", remote)

    def test_comb_prefix_remains_explicit(self):
        call = Cli(["comb", "--root", "/brain"])
        self.assertEqual(
            call.argv("search", "fact"), ["comb", "--root", "/brain", "search", "fact"]
        )

    def test_array_replies_require_opt_in_and_never_hide_command_failure(self):
        result = subprocess.CompletedProcess(["comb", "list"], 0, '[{"slug":"inbox/note"}]', "")
        call = Cli(["comb"])
        with patch("e2e.scripts.harness.cli.run_process", return_value=result):
            with self.assertRaisesRegex(RuntimeError, "unexpected response shape"):
                call("list")
            self.assertEqual(call("list", allow_list=True), [{"slug": "inbox/note"}])
            result.returncode = 1
            with self.assertRaises(RuntimeError):
                call("list", allow_list=True)
