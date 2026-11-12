"""Claude fixture controls on its single protocol runtime."""

import json
import os
import signal
import sys
import time
from pathlib import Path
from mock_common import (
    wait_for_gate,
    publish_text,
    append_line,
)


class Controls:
    def initialize_request(self, frame, request):
        if not self.session and self.scenario().get("catalog_gate"):
            release = Path(self.scenario()["catalog_gate"])
            publish_text(self.home / "catalog-pid", str(os.getpid()))
            wait_for_gate(release, timeout=30, timeout_error=RuntimeError("catalog gate timed out"))
        self.control(
            frame["request_id"],
            {
                "agents": [{"name": name} for name in ("general-purpose", "Explore", "Plan")],
                "models": [
                    {
                        "value": "sonnet",
                        "resolvedModel": "claude-sonnet-5",
                        "displayName": "Sonnet",
                        "description": "Deterministic mock",
                        "supportedEffortLevels": ["low", "medium", "high"],
                    },
                    {
                        "value": "haiku",
                        "resolvedModel": "claude-haiku-4-5",
                        "displayName": "Haiku",
                        "description": "Deterministic mock",
                        "supportedEffortLevels": ["low", "medium", "high"],
                    },
                ],
                "account": self.scenario().get(
                    "api_account",
                    (
                        {"apiProvider": "firstParty", "apiKeySource": "ANTHROPIC_API_KEY"}
                        if "ANTHROPIC_API_KEY" in os.environ
                        else {"subscriptionType": "Claude Team", "apiProvider": "firstParty"}
                    ),
                ),
            },
        )

    def settings_request(self, frame, request):
        if self.scenario().get("settings_gate"):
            release = Path(self.scenario()["settings_gate"])
            publish_text(self.home / "settings-waiting", str(os.getpid()))
            wait_for_gate(
                release, timeout=10, timeout_error=RuntimeError("settings gate timed out")
            )
        self.control(
            frame["request_id"],
            {
                "applied": {
                    "model": self.native_model,
                    "effort": (
                        "low"
                        if self.scenario().get("mismatch_effort")
                        else self.option("--effort")
                        if "--effort" in self.args
                        else None
                    ),
                }
            },
        )
        if self.scenario().get("backpressure"):
            prefix = os.read(sys.stdin.fileno(), 1)
            assert prefix == b"{"
            self.pending["backpressure"] = "backpressure"
            self.emit(
                {
                    "type": "control_request",
                    "request_id": "backpressure",
                    "request": {
                        "subtype": "mcp_message",
                        "server_name": "hive",
                        "message": {"jsonrpc": "2.0", "id": 42, "method": "ping"},
                    },
                }
            )
            time.sleep(0.2)
            self.queued_lines.append(prefix.decode() + sys.stdin.readline())

    def interrupt_request(self, frame, request):
        append_line(self.home / "interrupts.jsonl", json.dumps({"turn": self.turn}) + "\n")
        if self.turn and self.scenario().get("complete_before_interrupt"):
            self.finish()
        self.control(frame["request_id"], {"still_queued": [], "cancelled": []})
        if self.turn:
            if self.body == "native:agent-hold":
                self.emit(
                    {
                        "type": "user",
                        "session_id": self.session,
                        "message": {
                            "content": [
                                {
                                    "type": "tool_result",
                                    "tool_use_id": "agent-owner",
                                    "content": "interrupted",
                                    "is_error": True,
                                }
                            ]
                        },
                    }
                )
            if self.shell_process is not None:
                self.emit(
                    {
                        "type": "user",
                        "session_id": self.session,
                        "message": {
                            "content": [
                                {
                                    "type": "tool_result",
                                    "tool_use_id": "shell-tool",
                                    "content": "interrupted",
                                    "is_error": True,
                                }
                            ]
                        },
                    }
                )
            if self.mode in ("error-interrupt", "error-interrupt-exit"):
                self.emit(
                    {
                        "type": "result",
                        "session_id": self.session,
                        "subtype": "error_during_execution",
                        "is_error": True,
                        "num_turns": 1,
                    }
                )
                self.turn = None
                if self.mode == "error-interrupt-exit":
                    raise SystemExit(1)
                self.mode = None
            else:
                self.finish(interrupted=True)
        if self.shell_process is not None:
            time.sleep(0.15)  # native result may precede actual shell cleanup
            os.kill(int(Path("helper.pid").read_text()), signal.SIGTERM)
            self.shell_process.communicate(timeout=5)
            self.shell_process = None

    def handle_control_request(self, frame):
        request = frame["request"]
        if request["subtype"] == "initialize":
            self.initialize_request(frame, request)
        elif request["subtype"] == "get_settings":
            self.settings_request(frame, request)
        elif request["subtype"] == "interrupt":
            self.interrupt_request(frame, request)
        else:
            raise AssertionError(request)
