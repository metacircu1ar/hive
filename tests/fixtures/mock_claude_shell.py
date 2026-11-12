"""Claude fixture shell on its single protocol runtime."""

import json
import os
import signal
import subprocess
import time
from pathlib import Path
from mock_common import (
    publish_text,
)


class Shell:
    def reply_git(self, response):
        hook = response["response"]["hookSpecificOutput"]
        if hook["permissionDecision"] != "allow":
            result = "denied: " + hook["permissionDecisionReason"]
        else:
            command = hook["updatedInput"]["command"]
            shell = subprocess.run(
                [
                    os.environ["CLAUDE_CODE_SHELL"],
                    "-c",
                    command,
                ],
                capture_output=True,
                check=False,
            )
            result = "exit " + str(shell.returncode) + ": " + shell.stdout.decode().strip()
        self.emit(
            {
                "type": "user",
                "session_id": self.session,
                "parent_tool_use_id": None,
                "message": {
                    "content": [
                        {"type": "tool_result", "tool_use_id": "shell-tool", "content": result}
                    ]
                },
            }
        )
        self.complete("git " + result)
        return

    def reply_shell(self, response):
        hook = response["response"]["hookSpecificOutput"]
        assert hook["permissionDecision"] == "allow", hook
        command = hook["updatedInput"]["command"]
        shell_args = [
            os.environ["CLAUDE_CODE_SHELL"],
            "-c",
            command,
        ]
        if self.body in ("shell-killed", "shell-interrupt", "shell-retained"):
            phase = "start-helper"
            try:
                shell = subprocess.Popen(shell_args, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                helper, leader = self.wait_shell_ids(shell)
                if self.body == "shell-interrupt":
                    self.interrupt_shell(shell)
                    return
                phase = "kill-helper"
                os.kill(helper, signal.SIGKILL)
                if self.body == "shell-retained":
                    # Hold our known-live test group past the guard's 3s proof
                    # deadline, then explicitly clean it even if daemon stops.
                    # Uncertain Claude turns now receive TERM immediately; keep
                    # this fixture alive long enough to reap its retained group.
                    self.retain_shell()
                phase = "kill-group"
                os.killpg(leader, signal.SIGKILL)
                phase = "reap-helper"
                stdout, stderr = shell.communicate(timeout=5)
                publish_text(
                    Path("shell-diagnostic"),
                    json.dumps(
                        {
                            "phase": "reaped",
                            "helper": helper,
                            "leader": leader,
                            "exit": shell.returncode,
                            "stderr": stderr.decode(),
                        }
                    ),
                )
                assert shell.returncode != 0
                if self.body == "shell-retained":
                    publish_text(Path("retained-cleaned"), "")
                    signal.signal(signal.SIGTERM, signal.SIG_DFL)
                    return
            except BaseException as error:
                publish_text(
                    Path("shell-diagnostic"), json.dumps({"phase": phase, "error": repr(error)})
                )
                raise

        else:
            if self.body == "shell-sigchld":
                signal.signal(signal.SIGCHLD, signal.SIG_IGN)
            shell = subprocess.run(shell_args, capture_output=True, check=False)
            if self.body == "shell-sigchld":
                signal.signal(signal.SIGCHLD, signal.SIG_DFL)
            publish_text(
                Path("shell-diagnostic"), str(shell.returncode) + ":" + shell.stderr.decode()
            )
            assert shell.returncode == 0, (shell.returncode, shell.stderr)
        self.emit(
            {
                "type": "user",
                "session_id": self.session,
                "parent_tool_use_id": None,
                "message": {
                    "content": [
                        {"type": "tool_result", "tool_use_id": "shell-tool", "content": "done"}
                    ]
                },
            }
        )
        self.complete("shell-clean")
        return

    def wait_shell_ids(self, shell):
        deadline = time.monotonic() + 15
        while True:
            identities = self.read_shell_ids(shell, deadline)
            if identities is not None:
                return identities

    def read_shell_ids(self, shell, deadline):
        try:
            helper = int(Path("helper.pid").read_text())
            leader = int(Path("leader.pid").read_text())
            return helper, leader
        except (FileNotFoundError, ValueError):
            if shell.poll() is not None:
                stdout, stderr = shell.communicate()
                publish_text(
                    Path("shell-diagnostic"),
                    str(shell.returncode) + ":" + stderr.decode(),
                )
                raise AssertionError("helper exited before command started")
            assert time.monotonic() < deadline
            time.sleep(0.01)
        return None

    def interrupt_shell(self, shell):
        self.shell_process = shell
        self.emit(
            {
                "type": "assistant",
                "session_id": self.session,
                "parent_tool_use_id": None,
                "message": {
                    "model": self.native_model,
                    "content": [{"type": "text", "text": "shell-running"}],
                },
            }
        )

    def retain_shell(self):
        # Keep this fixture alive past the guard deadline so it can reap the
        # retained process group even after the daemon stops.
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        self.emit(
            {
                "type": "user",
                "session_id": self.session,
                "message": {
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": "shell-tool",
                            "content": "native returned",
                        }
                    ]
                },
            }
        )
        self.finish()
        time.sleep(4)
