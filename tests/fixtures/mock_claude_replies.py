"""Claude fixture replies on its single protocol runtime."""

import json
from pathlib import Path
from mock_common import (
    publish_text,
)


class Replies:
    def reply_native_agent(self, response):
        hook = response["response"]["hookSpecificOutput"]
        assert hook["permissionDecision"] == "allow", hook
        assert hook["updatedInput"]["run_in_background"] is False
        self.emit(
            {
                "type": "system",
                "subtype": "task_started",
                "session_id": self.session,
                "task_id": "private-native-task",
                "tool_use_id": "agent-owner",
                "task_type": "local_agent",
                "spawn_depth": 1,
                "is_backgrounded": False,
            }
        )
        self.emit(
            {
                "type": "assistant",
                "session_id": self.session,
                "parent_tool_use_id": "agent-owner",
                "message": {
                    "model": self.native_model,
                    "content": [{"type": "text", "text": "PRIVATE_NATIVE_REPLY"}],
                },
            }
        )
        if self.body == "native:agent-hold":
            publish_text(self.home / "native-agent-ready", "ready")
        else:
            self.emit(
                {
                    "type": "user",
                    "session_id": self.session,
                    "message": {
                        "content": [
                            {
                                "type": "tool_result",
                                "tool_use_id": "agent-owner",
                                "content": "Private task finished",
                            }
                        ]
                    },
                }
            )
            self.complete("native-parent-result")
        return

    def reply_backpressure(self, response):
        assert response["response"]["mcp_response"]["id"] == 42
        assert response["response"]["mcp_response"]["result"] == {}
        publish_text(self.home / "backpressure-confirmed", "")
        return

    def reply_skill(self, response):
        if response["response"]["hookSpecificOutput"]["permissionDecision"] != "allow":
            self.emit(
                {
                    "type": "user",
                    "session_id": self.session,
                    "message": {
                        "content": [
                            {
                                "type": "tool_result",
                                "tool_use_id": "skill-tool",
                                "content": "denied",
                                "is_error": True,
                            }
                        ]
                    },
                }
            )
            self.complete("skill-denied")
            return
        self.emit(
            {
                "type": "system",
                "subtype": "task_started",
                "session_id": self.session,
                "task_id": "skill-task",
                "tool_use_id": "skill-tool",
                "description": "/code-review",
                "is_backgrounded": False,
                "task_type": "local_agent",
                "spawn_depth": 1,
            }
        )
        self.emit(
            {
                "type": "user",
                "session_id": self.session,
                "parent_tool_use_id": "skill-tool",
                "isSynthetic": True,
                "message": {"content": [{"type": "text", "text": "Review the diff."}]},
            }
        )
        self.start_skill_read()

    def start_skill_read(self):
        read_input = {"file_path": str(Path.cwd() / "reviewed.txt")}
        self.emit(
            {
                "type": "assistant",
                "session_id": self.session,
                "parent_tool_use_id": "skill-tool",
                "message": {
                    "model": self.native_model,
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "skill-read",
                            "name": "Read",
                            "input": read_input,
                        }
                    ],
                },
            }
        )
        self.pending["skill-read-hook"] = "skill-read"
        self.emit(
            {
                "type": "control_request",
                "request_id": "skill-read-hook",
                "request": {
                    "subtype": "hook_callback",
                    "callback_id": "hive-guard",
                    "input": {
                        "hook_event_name": "PreToolUse",
                        "session_id": self.session,
                        "tool_use_id": "skill-read",
                        "tool_name": "Read",
                        "agent_id": "skill-task",
                        "tool_input": read_input,
                    },
                },
            }
        )
        return

    def reply_skill_read(self, response):
        assert response["response"]["hookSpecificOutput"]["permissionDecision"] == "allow"
        self.emit(
            {
                "type": "user",
                "session_id": self.session,
                "parent_tool_use_id": "skill-tool",
                "message": {
                    "content": [
                        {"type": "tool_result", "tool_use_id": "skill-read", "content": "read"}
                    ]
                },
            }
        )
        self.emit(
            {
                "type": "tool_progress",
                "session_id": self.session,
                "parent_tool_use_id": "skill-tool",
                "tool_use_id": "skill-tool-heartbeat-0",
                "tool_name": "Skill",
                "heartbeat": True,
            }
        )
        self.emit(
            {
                "type": "system",
                "subtype": "task_notification",
                "session_id": self.session,
                "task_id": "skill-task",
                "tool_use_id": "skill-tool",
                "status": "completed",
            }
        )
        self.emit(
            {
                "type": "user",
                "session_id": self.session,
                "message": {
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": "skill-tool",
                            "content": 'Skill "code-review" completed (forked execution).',
                        }
                    ]
                },
            }
        )
        self.complete("skill-forked")
        return

    def handle_control_response(self, frame):
        if frame["response"]["subtype"] == "success":
            self.emit(frame)  # native only echoes tracked successful control replies
        response = frame["response"]
        continuation = self.pending.pop(response["request_id"])
        handlers = {
            "native-agent": self.reply_native_agent,
            "backpressure": self.reply_backpressure,
            "skill": self.reply_skill,
            "skill-read": self.reply_skill_read,
            "git": self.reply_git,
            "shell": self.reply_shell,
        }
        handler = handlers.get(continuation)
        if handler is not None:
            handler(response)
            return
        result = response["response"]["mcp_response"]["result"]
        if continuation == "comb-catalog":
            self.comb_results.append(result)
            self.comb_next()
            return
        if continuation == "comb-result":
            self.comb_results.append(json.loads(result["content"][0]["text"]))
            self.comb_next()
            return
        assert not result["isError"], result
        if continuation == "spawned":
            child = json.loads(result["content"][0]["text"])["agent_id"]
            self.complete("reviewer:" + child)
        else:
            self.finish()
        return
