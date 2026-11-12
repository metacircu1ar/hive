"""One native Claude process; state survives tool callbacks and turns."""

import json
from mock_common import (
    load_scenario,
)

from mock_claude_startup import Startup
from mock_claude_controls import Controls
from mock_claude_input import Input
from mock_claude_actions import Actions
from mock_claude_tasks import Tasks
from mock_claude_replies import Replies
from mock_claude_shell import Shell


class ClaudeRuntime(Startup, Controls, Input, Actions, Tasks, Replies, Shell):
    def __init__(self, home, args):
        self.home = home
        self.args = args
        self.initialize()

    def scenario(self):
        # Test commands can change settings while this native process remains resident.
        return load_scenario(self.home / "runtime-scenario.json")

    def option(self, name):
        return self.args[self.args.index(name) + 1]

    def emit(self, frame):
        if frame.get("type") in ("user", "assistant", "stream_event"):
            frame.setdefault("parent_tool_use_id", None)
        print(json.dumps(frame), flush=True)

    def control(self, request, response):
        self.emit(
            {
                "type": "control_response",
                "response": {"subtype": "success", "request_id": request, "response": response},
            }
        )

    def finish(self, interrupted=False):
        self.emit(
            {
                "type": "result",
                "session_id": self.session,
                "subtype": "success",
                "is_error": False,
                "terminal_reason": "aborted_tools" if interrupted else "completed",
                "user_message_uuid": self.turn,
                **(
                    {"modelUsage": self.scenario()["model_usage"]}
                    if "model_usage" in self.scenario()
                    else {}
                ),
            }
        )
        self.turn = None

    def tool(self, name, arguments, continuation, native_id=None):
        self.counter += 1
        outer = "control-" + str(self.counter) if native_id is None else "comb-" + str(native_id)
        self.pending[outer] = continuation
        self.emit(
            {
                "type": "control_request",
                "request_id": outer,
                "request": {
                    "subtype": "mcp_message",
                    "server_name": "hive",
                    "message": {
                        "jsonrpc": "2.0",
                        "id": "mcp-" + str(self.counter)
                        if native_id is None
                        else "comb-mcp-" + str(native_id),
                        "method": "tools/call",
                        "params": {"name": name, "arguments": arguments},
                    },
                },
            }
        )

    def comb_next(self):
        if self.comb_actions:
            action = self.comb_actions.pop(0)
            if action.get("catalog"):
                self.counter += 1
                outer = "comb-catalog-" + str(self.counter)
                self.pending[outer] = "comb-catalog"
                self.emit(
                    {
                        "type": "control_request",
                        "request_id": outer,
                        "request": {
                            "subtype": "mcp_message",
                            "server_name": "hive",
                            "message": {
                                "jsonrpc": "2.0",
                                "id": self.counter,
                                "method": "tools/list",
                                "params": {},
                            },
                        },
                    }
                )
                return
            self.tool(
                action["name"], action.get("arguments", {}), "comb-result", action.get("native_id")
            )
        else:
            self.complete("comb-results:" + json.dumps(self.comb_results))

    def complete(self, text):
        message = {"model": self.native_model, "content": [{"type": "text", "text": text}]}
        if self.scenario().get("live_deltas"):
            # Partial stream events precede the aggregated assistant message.
            message["id"] = "msg_" + str(self.turn)

            def partial(event):
                self.emit(
                    {
                        "type": "stream_event",
                        "session_id": self.session,
                        "parent_tool_use_id": None,
                        "uuid": self.turn,
                        "event": event,
                    }
                )

            partial(
                {
                    "type": "message_start",
                    "message": {"model": self.native_model, "id": message["id"]},
                }
            )
            chunk = self.scenario().get("delta_chars", 3)
            partial(
                {
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "thinking_delta", "thinking": "Planning."},
                }
            )
            for start in range(0, len(text), chunk):
                partial(
                    {
                        "type": "content_block_delta",
                        "index": 1,
                        "delta": {"type": "text_delta", "text": text[start : start + chunk]},
                    }
                )
            partial(
                {
                    "type": "content_block_delta",
                    "index": 1,
                    "delta": {"type": "input_json_delta", "partial_json": "{}"},
                }
            )
        self.emit(
            {
                "type": "assistant",
                "session_id": self.session,
                "parent_tool_use_id": None,
                "message": message,
            }
        )
        self.finish()
