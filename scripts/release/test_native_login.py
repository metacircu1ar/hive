"""Opt-in native setup/login cancellation through an isolated mocked provider."""

import errno
import json
import os
from pathlib import Path
import signal
import sys
import time
import unittest
from .common import ROOT
from .files import atomic_copy, atomic_write_text, create_directory
from .owned_process import spawn
from .temporary import directory
from .cancellation import pause
from .terminal import terminal


@unittest.skipUnless(
    os.environ.get("HIVE_TEST_SETUP_LOGIN"), "Requires a selected plain release Hive binary"
)
class NativeLoginTests(unittest.TestCase):
    def test_setup_owner_reaps_separately_grouped_uncooperative_login_before_forced_exit(self):
        binary = Path(os.environ["HIVE_TEST_SETUP_LOGIN"]).resolve()
        self.assertEqual(binary.parent.name, "release")
        parent = ROOT / "dist/native-login-fixtures"
        create_directory(parent, exist_ok=True)
        with directory(prefix="hive-login-", parent=parent) as root:
            pid_file = root / "provider.pid"
            scenario = root / "scenario.json"
            atomic_write_text(scenario, json.dumps({"login_ignore_signals": True}), mode=0o600)
            for name in ("mock_common.py", "codex-schema.json"):
                atomic_copy(ROOT / "tests/fixtures" / name, root / name, mode=0o600)
            atomic_copy(ROOT / "scripts/release/files.py", root / "mock_files.py", mode=0o600)
            body = (ROOT / "tests/fixtures/provider.py").read_text()
            needle = '    print("fake-login: enter confirmation", flush=True)'
            self.assertEqual(body.count(needle), 1)
            body = body.replace(
                needle,
                "    signal.signal(signal.SIGHUP,signal.SIG_IGN)\n"
                + f"    pathlib.Path({json.dumps(str(pid_file))}).write_text(str(os.getpid()))\n"
                + needle,
            )
            provider = root / "provider"
            atomic_write_text(
                provider,
                f"#!{sys.executable}\nSCENARIO={json.dumps(str(scenario))}\n"
                + "PROVIDER='codex'\n"
                + body,
                mode=0o700,
            )
            env = {
                key: value
                for key, value in os.environ.items()
                if not key.startswith("HIVE_")
                and key
                not in {
                    "OPENAI_API_KEY",
                    "ANTHROPIC_API_KEY",
                    "CLAUDE_CODE_OAUTH_TOKEN",
                    "CODEX_API_KEY",
                }
            }
            env.update(
                HOME=str(root),
                XDG_CONFIG_HOME=str(root / "cfg"),
                XDG_STATE_HOME=str(root / "st"),
                XDG_DATA_HOME=str(root / "data"),
            )
            command = [
                str(binary),
                "--state-dir",
                str(root / "state"),
                "--config",
                str(root / "config.yaml"),
                "--codex-bin",
                str(provider),
                "setup",
                "--provider",
                "codex",
                "--login",
                "browser",
                "--codex-model",
                "fixture-default",
                "--codex-effort",
                "low",
                "--no-comb",
                "--no-services",
            ]
            self.exercise(command, env, pid_file)

    def exercise(self, command, env, pid_file):
        launch = (
            "import fcntl,os,sys,termios;fcntl.ioctl(0,termios.TIOCSCTTY,0);"
            "os.execv(sys.argv[1],sys.argv[1:])"
        )
        with terminal() as (master, slave, retire_capture):
            with spawn(
                [sys.executable, "-c", launch, *command],
                env=env,
                stdin=slave,
                stdout=slave,
                stderr=slave,
                stop_grace=12,
                capture_fd=master,
                capture_close=retire_capture,
                limit=1024 * 1024,
            ) as owner:
                until = time.monotonic() + 10
                while not pid_file.exists() and time.monotonic() < until:
                    self.assertFalse(owner.exited(), owner.output.decode(errors="replace"))
                    pause(0.02)
                self.assertTrue(pid_file.exists(), owner.output.decode(errors="replace"))
                pid = int(pid_file.read_text())
                began = time.monotonic()
                owner.stop()
                self.assertNotEqual(owner.returncode, -signal.SIGKILL)
                self.assertLess(time.monotonic() - began, 12)
                with self.assertRaises(ProcessLookupError) as gone:
                    os.kill(pid, 0)
                self.assertEqual(gone.exception.errno, errno.ESRCH)
