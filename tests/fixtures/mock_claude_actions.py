"""Claude fixture actions on its single protocol runtime."""

import json
import os
import socket
from mock_common import (
    publish_text,
)


class Actions:
    def body_native_agent(self):
        native_input = {
            "description": "Private fixture",
            "prompt": "Private work",
            "subagent_type": "general-purpose",
            "run_in_background": False,
        }
        self.emit(
            {
                "type": "assistant",
                "session_id": self.session,
                "message": {
                    "model": self.native_model,
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "agent-owner",
                            "name": "Agent",
                            "input": native_input,
                        }
                    ],
                },
            }
        )
        self.pending["native-agent-hook"] = "native-agent"
        self.emit(
            {
                "type": "control_request",
                "request_id": "native-agent-hook",
                "request": {
                    "subtype": "hook_callback",
                    "callback_id": "hive-guard",
                    "input": {
                        "hook_event_name": "PreToolUse",
                        "session_id": self.session,
                        "tool_use_id": "agent-owner",
                        "tool_name": "Agent",
                        "tool_input": native_input,
                    },
                },
            }
        )
        return

    def body_skill(self):
        publish_text(
            self.home / "skill-launch.json",
            json.dumps(
                {
                    "disable_slash_commands": "--disable-slash-commands" in self.args,
                    "tools": self.option("--tools"),
                }
            ),
        )
        skill_input = {"skill": "code-review"}
        self.emit(
            {
                "type": "assistant",
                "session_id": self.session,
                "parent_tool_use_id": None,
                "message": {
                    "model": self.native_model,
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "skill-tool",
                            "name": "Skill",
                            "input": skill_input,
                        }
                    ],
                },
            }
        )
        self.pending["skill-hook"] = "skill"
        self.emit(
            {
                "type": "control_request",
                "request_id": "skill-hook",
                "request": {
                    "subtype": "hook_callback",
                    "callback_id": "hive-guard",
                    "input": {
                        "hook_event_name": "PreToolUse",
                        "session_id": self.session,
                        "tool_use_id": "skill-tool",
                        "tool_name": "Skill",
                        "tool_input": skill_input,
                    },
                },
            }
        )
        return

    def body_shell(self):
        self.pending["shell-hook"] = "shell"
        shell_body = 'sleep 60 & child=$!; kill -0 "$child" && echo "$child" > background.pid'
        if self.body in ("shell-killed", "shell-interrupt", "shell-retained"):
            shell_body = 'echo "$PPID" > helper.pid; echo "$$" > leader.pid; exec sleep 60'
        if self.body == "shell-stray":
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stray:
                stray.connect(os.environ["HIVE_SHELL_GUARD_SOCKET"])
                try:
                    stray.sendall(b"not a registered helper\n")
                except (BrokenPipeError, ConnectionResetError):
                    # Peer authentication may reject this socket before its
                    # malformed payload is written. Both timings are valid.
                    pass
        self.emit(
            {
                "type": "assistant",
                "session_id": self.session,
                "parent_tool_use_id": None,
                "message": {
                    "model": self.native_model,
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "shell-tool",
                            "name": "Bash",
                            "input": {"command": shell_body},
                        }
                    ],
                },
            }
        )
        self.emit(
            {
                "type": "control_request",
                "request_id": "shell-hook",
                "request": {
                    "subtype": "hook_callback",
                    "callback_id": "hive-guard",
                    "input": {
                        "hook_event_name": "PreToolUse",
                        "session_id": self.session,
                        "tool_use_id": "shell-tool",
                        "tool_name": "Bash",
                        "tool_input": {"command": shell_body},
                    },
                },
            }
        )
        return

    def body_git(self):
        self.pending["shell-hook"] = "git"
        git_command = "git status --porcelain"
        if self.body == "git-denied":
            git_command = "git status; touch pwned"
        self.emit(
            {
                "type": "assistant",
                "session_id": self.session,
                "parent_tool_use_id": None,
                "message": {
                    "model": self.native_model,
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "shell-tool",
                            "name": "Bash",
                            "input": {"command": git_command},
                        }
                    ],
                },
            }
        )
        self.emit(
            {
                "type": "control_request",
                "request_id": "shell-hook",
                "request": {
                    "subtype": "hook_callback",
                    "callback_id": "hive-guard",
                    "input": {
                        "hook_event_name": "PreToolUse",
                        "session_id": self.session,
                        "tool_use_id": "shell-tool",
                        "tool_name": "Bash",
                        "tool_input": {"command": git_command},
                    },
                },
            }
        )
        return
