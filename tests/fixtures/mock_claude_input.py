"""Claude fixture input on its single protocol runtime."""

import json
import os
import time
import uuid
from pathlib import Path
from mock_common import (
    wait_for_gate,
    publish_text,
    append_line,
    INPUT_RECORD_LIMIT,
)


class Input:
    def read_user(self, frame):
        assert self.turn is None
        self.turn = frame["uuid"]
        assert frame["session_id"] == self.session
        publish_text(self.saved, "")
        if self.scenario().get("input_gate"):
            release = Path(self.scenario()["input_gate"])
            publish_text(self.home / "input-waiting", str(os.getpid()))
            wait_for_gate(release, timeout=10, timeout_error=RuntimeError("input gate timed out"))
        self.emit(
            {
                "type": "system",
                "subtype": "init",
                "session_id": self.session,
                "model": self.native_model,
                "permissionMode": "default",
                "capabilities": [
                    "interrupt_receipt_v1",
                    "interrupt_cancel_queued_v1",
                    "msg_lifecycle_v1",
                ],
            }
        )
        self.emit(
            {
                "type": "user",
                "uuid": self.turn,
                "session_id": self.session,
                "parent_tool_use_id": None,
                "isReplay": True,
            }
        )
        content = frame["message"]["content"]
        text = (
            content
            if isinstance(content, str)
            else "\n".join(block["text"] for block in content if block["type"] == "text")
        )
        self.body = text.split("\n", 1)[1]
        append_line(
            self.home / "inputs.jsonl",
            json.dumps(
                {
                    "thread": self.session,
                    "body": self.body,
                    "input": text,
                    "native_input": content,
                    "at": time.time(),
                }
            )
            + "\n",
            max_record_bytes=INPUT_RECORD_LIMIT,
        )
        if self.body.startswith("Hive branch identity:"):
            assert self.fork, "branch initial input must belong to native --fork-session"
            assert f"You are agent {self.agent}," in self.body
            self.body = self.body.split("\n\nNew task:\n", 1)[1]
        self.history.append(self.body)
        publish_text(self.history_file, json.dumps(self.history))

    def record_native_history(self):
        self.body = self.body.split("\n\nHive attachment references", 1)[0]
        parent = self.native_history[-1]["uuid"] if self.native_history else None
        if self.fork and self.scenario().get("branch_disconnected_history"):
            parent = None
        self.native_history.extend(
            [
                {
                    "uuid": self.turn,
                    "parentUuid": parent,
                    "type": "user",
                    "sessionId": self.session,
                    "message": {"role": "user", "content": self.body},
                },
                {
                    "uuid": str(uuid.uuid4()),
                    "parentUuid": self.turn,
                    "type": "assistant",
                    "sessionId": self.session,
                    "message": {"role": "assistant", "content": [{"type": "text", "text": "OK"}]},
                },
            ]
        )
        if self.body in ("native:agent", "native:agent-hold"):
            self.native_history.insert(
                -1,
                {
                    "uuid": str(uuid.uuid4()),
                    "parentUuid": self.turn,
                    "type": "assistant",
                    "isSidechain": True,
                    "sessionId": self.session,
                    "message": {"role": "assistant", "content": "PRIVATE_NATIVE_REPLY"},
                },
            )
        publish_text(
            self.native_history_file, "".join(json.dumps(row) + "\n" for row in self.native_history)
        )

    def quota_once(self):
        if self.scenario().get("quota_once") and not (self.home / "quota-seen").exists():
            publish_text(self.home / "quota-seen", "1")
            self.emit(
                {
                    "type": "rate_limit_event",
                    "session_id": self.session,
                    "rate_limit_info": {
                        "status": "rejected",
                        "resetsAt": int(time.time()) + 3600,
                        "rateLimitType": "five_hour",
                    },
                }
            )
            self.emit(
                {
                    "type": "assistant",
                    "session_id": self.session,
                    "error": "rate_limit",
                    "message": {"model": self.native_model, "content": []},
                }
            )
            self.emit(
                {
                    "type": "result",
                    "session_id": self.session,
                    "subtype": "error_during_execution",
                    "is_error": True,
                    "user_message_uuid": self.turn,
                    "num_turns": 1,
                }
            )
            self.turn = None
            return True
        return False

    def handle_user(self, frame):
        self.read_user(frame)
        self.record_native_history()
        if self.quota_once():
            return
        if self.scenario().get("usage_info"):
            self.emit(
                {
                    "type": "rate_limit_event",
                    "session_id": self.session,
                    "rate_limit_info": self.scenario()["usage_info"],
                }
            )
        self.dispatch_body()
