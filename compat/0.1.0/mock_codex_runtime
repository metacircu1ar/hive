"""One native Codex process and its resident conversation routing."""

import json
import subprocess
import sys
from mock_common import (
    wait_for_gate,
    publish_text,
    append_line,
)
from mock_codex_conversation import Conversation

from mock_codex_account import Account
from mock_codex_threads import Threads
from mock_codex_turns import Turns
from mock_codex_replies import Replies
from mock_codex_bodies import Bodies


class CodexRuntime(Account, Threads, Turns, Replies, Bodies):
    def __init__(self, home, scenario):
        self.home = home
        self.scenario = scenario
        self.conversations = {}
        self.callbacks = {}
        self.loaded_threads = set()
        self.api_key = None
        self.handlers = {
            "initialize": self.handle_initialize,
            "account/read": self.handle_account,
            "account/rateLimits/read": self.handle_rate_limits,
            "model/list": self.handle_models,
            "thread/start": self.handle_thread,
            "thread/resume": self.handle_thread,
            "thread/fork": self.handle_thread,
            "turn/start": self.handle_turn_start,
            "turn/steer": self.handle_steer,
            "turn/interrupt": self.handle_interrupt,
            "thread/backgroundTerminals/clean": self.handle_clean,
            "thread/unsubscribe": self.handle_unsubscribe,
            "thread/loaded/list": self.handle_loaded_threads,
            "thread/backgroundTerminals/list": self.handle_terminals,
        }
        self.handlers.update(
            {
                "configRequirements/read": self.handle_requirements,
                "config/read": self.handle_config,
                "account/login/start": self.handle_api_login,
            }
        )

    def select_conversation(self, request, conversation):
        # Commands from different resident conversations and tool replies may interleave.
        if conversation.thread_id is not None:
            self.conversations[conversation.thread_id] = conversation
        method = request.get("method")
        params = request.get("params", {})
        if method in ("thread/start", "thread/fork"):
            return Conversation()
        if method == "thread/resume":
            return Conversation(thread_id=params["threadId"])
        if "threadId" in params:
            return self.conversations[params["threadId"]]
        if method is None and "result" in request:
            return self.conversations[self.callbacks.pop(request["id"])]
        return conversation

    def emit(self, value):
        print(json.dumps(value), flush=True)

    def reply(self, request, result):
        self.emit({"id": request["id"], "result": result})

    def gated_ack(self, request, turn, gate):
        if wait_for_gate(gate, timeout=10, strict_deadline=True):
            self.reply(request, {"turnId": turn})

    def save(self, conversation):
        publish_text(self.home / (conversation.thread_id + ".json"), json.dumps(conversation.meta))

    def finish(self, conversation, status="completed"):
        conversation.active_turn = False
        conversation.meta["last_turn"] = conversation.turn_id
        self.save(conversation)
        self.emit(
            {
                "method": "turn/completed",
                "params": {
                    "threadId": conversation.thread_id,
                    "turn": {"id": conversation.turn_id, "status": status},
                },
            }
        )

    def late_terminal(self, conversation):
        assert conversation.command_child is None
        conversation.command_child = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(120)"]
        )
        publish_text(
            self.home / ("late-shell-" + conversation.meta["agent"]),
            str(conversation.command_child.pid),
        )
        self.emit(
            {
                "method": "item/started",
                "params": {
                    "threadId": conversation.thread_id,
                    "turnId": conversation.turn_id,
                    "item": {"id": "late-command", "type": "commandExecution"},
                },
            }
        )

    def call(self, conversation, name, args, native_id=None):
        conversation.pending = name
        conversation.pending_args = args
        tool_id = "tool-" + conversation.turn_id if native_id is None else "comb-" + str(native_id)
        self.callbacks[tool_id] = conversation.thread_id
        self.emit(
            {
                "id": tool_id,
                "method": "item/tool/call",
                "params": {
                    "threadId": conversation.thread_id,
                    "turnId": conversation.turn_id,
                    "callId": "call-" + tool_id,
                    "tool": name,
                    "arguments": args,
                },
            }
        )

    def handle_initialize(self, request, conversation):
        if self.scenario.get("record_initialize"):
            append_line(
                self.home / "initialize.jsonl", json.dumps(request.get("params", {})) + "\n"
            )
        self.reply(request, {})

    def handle_unsupported(self, request, conversation):
        self.emit({"id": request["id"], "error": {"code": -32601, "message": "unsupported"}})
