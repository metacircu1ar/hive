"""Codex fixture turns on its single protocol runtime."""

import json
import pathlib
import sys
import threading
import time
import uuid
from mock_common import (
    wait_for_gate,
    publish_text,
    append_line,
    INPUT_RECORD_LIMIT,
)


class Turns:
    def stream_live(self, conversation, item_id, text):
        # Opt-in live mode: a command with output, a reasoning summary, then the
        # answer as small deltas, as native Codex streams them before item/completed.
        base = {"threadId": conversation.thread_id, "turnId": conversation.turn_id}
        delay = self.scenario.get("delta_delay", 0)
        command = {
            "type": "commandExecution",
            "id": "cmd-" + conversation.turn_id,
            "command": "echo live",
            "cwd": ".",
            "commandActions": [],
            "status": "inProgress",
        }
        self.emit({"method": "item/started", "params": {**base, "item": command}})
        self.emit(
            {
                "method": "item/commandExecution/outputDelta",
                "params": {**base, "itemId": command["id"], "delta": "live\n"},
            }
        )
        self.emit(
            {
                "method": "item/completed",
                "params": {**base, "item": {**command, "status": "completed", "exitCode": 0}},
            }
        )
        self.emit(
            {
                "method": "item/reasoning/summaryTextDelta",
                "params": {
                    **base,
                    "itemId": "rs-" + conversation.turn_id,
                    "summaryIndex": 0,
                    "delta": "Planning the answer.",
                },
            }
        )
        # Raw reasoning is opted out; a provider that sends it anyway must be ignored.
        self.emit(
            {
                "method": "item/reasoning/textDelta",
                "params": {
                    **base,
                    "itemId": "rs-" + conversation.turn_id,
                    "contentIndex": 0,
                    "delta": "raw",
                },
            }
        )
        self.emit(
            {
                "method": "item/started",
                "params": {
                    **base,
                    "item": {
                        "type": "userMessage",
                        "id": "echo-" + conversation.turn_id,
                        "content": [{"type": "text", "text": "echoed input"}],
                    },
                },
            }
        )
        chunk = self.scenario.get("delta_chars", 3)
        for start in range(0, len(text), chunk):
            self.emit(
                {
                    "method": "item/agentMessage/delta",
                    "params": {**base, "itemId": item_id, "delta": text[start : start + chunk]},
                }
            )
            if delay:
                time.sleep(delay)

    def handle_turn_start(self, request, conversation):
        p = request.get("params", {})
        if self.scenario.get("reject_turn"):
            if self.scenario.get("rejected_but_started"):
                self.emit(
                    {
                        "method": "turn/started",
                        "params": {
                            "threadId": conversation.thread_id,
                            "turn": {"id": "unexpected", "status": "inProgress"},
                        },
                    }
                )
            self.emit({"id": request["id"], "error": {"code": -1, "message": "turn rejected"}})
            return
        append_line(
            self.home / "turn-selections.jsonl",
            json.dumps(
                {
                    "thread": conversation.thread_id,
                    "model": p["model"],
                    "effort": p["effort"],
                    "summary": p.get("summary"),
                }
            )
            + "\n",
        )
        if self.scenario.get("persist_turn_selection"):
            conversation.meta["model"], conversation.meta["effort"] = p["model"], p["effort"]
            self.save(conversation)
        if self.scenario.get("turn_start_gate"):
            gate = pathlib.Path(self.scenario["turn_start_gate"])
            wait_for_gate(gate, timeout=10, timeout_error=RuntimeError("turn start gate timed out"))
        conversation.turn_id = str(uuid.uuid4())
        conversation.active_turn = True
        if self.scenario.get("missing_turn_ack"):
            self.reply(request, {"turn": {}})
            return
        self.reply(request, {"turn": {"id": conversation.turn_id}})
        self.emit(
            {
                "method": "turn/started",
                "params": {
                    "threadId": conversation.thread_id,
                    "turn": {"id": conversation.turn_id, "status": "inProgress"},
                },
            }
        )
        body, sender = self.record_input(conversation, p)
        if self.scenario.get("echo_input_items"):
            item = {"id": str(uuid.uuid4()), "type": "userMessage", "content": p["input"]}
            for method in ("item/started", "item/completed"):
                self.emit(
                    {
                        "method": method,
                        "params": {
                            "threadId": conversation.thread_id,
                            "turnId": conversation.turn_id,
                            "item": item,
                        },
                    }
                )
        if self.quota_error(conversation, body):
            return
        if self.scenario.get("rate_limits_update"):
            # A sparse rolling update, as Codex sends during turns.
            self.emit(
                {
                    "method": "account/rateLimits/updated",
                    "params": {"rateLimits": self.scenario["rate_limits_update"]},
                }
            )
        self.handle_body(conversation, body, sender)

    def handle_steer(self, request, conversation):
        p = request.get("params", {})
        if not conversation.active_turn and self.scenario.get("probe_gate"):
            wait_for_gate(
                self.scenario["probe_gate"],
                timeout=10,
                timeout_error=AssertionError("Probe gate was never released"),
            )
        if self.validate_steer(request, conversation, p):
            return
        self.steer_active(request, conversation, p)

    def validate_steer(self, request, conversation, p):
        if self.scenario.get("empty_input_first") and (not p["input"]):
            self.emit(
                {
                    "id": request["id"],
                    "error": {"code": -32600, "message": "input must not be empty"},
                }
            )
            return True
        if self.scenario.get("probe_stale") and (not conversation.active_turn):
            self.emit(
                {
                    "id": request["id"],
                    "error": {
                        "code": -32600,
                        "message": (
                            f"expected active turn id `{p['expectedTurnId']}` but found `untracked`"
                        ),
                    },
                }
            )
            return True
        if self.scenario.get("probe_success") and (not conversation.active_turn):
            self.reply(request, {"turnId": p["expectedTurnId"]})
            return True
        return self.reject_inactive_steer(request, conversation, p)

    def reject_inactive_steer(self, request, conversation, p):
        if self.scenario.get("no_steering"):
            self.emit({"id": request["id"], "error": {"code": -32601, "message": "unsupported"}})
            return True
        if not conversation.active_turn:
            message = (
                "different validation failure"
                if self.scenario.get("unknown_steering_probe")
                else "no active turn to steer"
            )
            self.emit({"id": request["id"], "error": {"code": -32600, "message": message}})
            return True
        if p["expectedTurnId"] != conversation.turn_id:
            self.emit(
                {
                    "id": request["id"],
                    "error": {
                        "code": -32600,
                        "message": (
                            f"expected active turn id `{p['expectedTurnId']}` "
                            f"but found `{conversation.turn_id}`"
                        ),
                    },
                }
            )
            return True
        return False

    def steer_active(self, request, conversation, p):
        if self.scenario.get("stale_steer"):
            self.finish(conversation)
            self.emit(
                {
                    "id": request["id"],
                    "error": {"code": -32600, "message": "no active turn to steer"},
                }
            )
        elif self.scenario.get("reject_steer") or self.scenario.get("uncertain_steer"):
            code = -32603 if self.scenario.get("uncertain_steer") else -32600
            self.emit({"id": request["id"], "error": {"code": code, "message": "steering failed"}})
        elif self.scenario.get("exit_steer"):
            sys.exit(0)
        elif self.scenario.get("steer_ack_after_tool"):
            conversation.steering_request = request
            self.call(conversation, "hive_status", {"target": conversation.meta["agent"]})
        elif self.scenario.get("steer_ack_after_interrupt"):
            conversation.steering_request = request
            publish_text(pathlib.Path(self.scenario["steer_ack_after_interrupt"]), "received")
        else:
            append_line(
                self.home / "steers.jsonl",
                json.dumps(p) + "\n",
                max_record_bytes=INPUT_RECORD_LIMIT,
            )
            body = p["input"][0]["text"].split(":\n", 1)[1]
            if self.scenario.get("finish_before_steer_ack"):
                self.finish(conversation)
            self.reply(
                request,
                {
                    "turnId": "wrong-turn"
                    if self.scenario.get("wrong_steer_ack")
                    else conversation.turn_id
                },
            )
            self.emit(
                {
                    "method": "item/completed",
                    "params": {
                        "threadId": conversation.thread_id,
                        "turnId": conversation.turn_id,
                        "item": {"type": "agentMessage", "text": "steered: " + body},
                    },
                }
            )
            if body in ("finish", "self guidance") and conversation.active_turn:
                self.finish(conversation)

    def handle_interrupt(self, request, conversation):
        p = request.get("params", {})
        append_line(self.home / "interrupts.jsonl", json.dumps(p) + "\n")
        if self.scenario.get("complete_before_interrupt") and conversation.active_turn:
            self.finish(conversation)
        if not conversation.active_turn or p["turnId"] != conversation.turn_id:
            self.emit(
                {
                    "id": request["id"],
                    "error": {"code": -32600, "message": "no matching active turn"},
                }
            )
            return None
        if self.scenario.get("reject_interrupt"):
            self.emit({"id": request["id"], "error": {"code": -1, "message": "already ending"}})
            time.sleep(0.1)
        else:
            self.reply(request, {})
        if self.scenario.get("wrong_completion"):
            self.emit(
                {
                    "method": "turn/completed",
                    "params": {
                        "threadId": conversation.thread_id,
                        "turn": {"id": "wrong-turn", "status": "interrupted"},
                    },
                }
            )
            return False
        if conversation.command_child and not self.scenario.get("retain_shell_on_interrupt"):
            conversation.command_child.terminate()
            conversation.command_child.wait()
            conversation.command_child = None
        self.finish(conversation, "interrupted")
        if self.scenario.get("late_immediate"):
            self.late_terminal(conversation)
        if conversation.steering_request is not None:
            if self.scenario.get("steer_ack_gate"):
                threading.Thread(
                    target=self.gated_ack,
                    args=(
                        conversation.steering_request,
                        conversation.turn_id,
                        self.scenario["steer_ack_gate"],
                    ),
                    daemon=True,
                ).start()
            else:
                self.reply(conversation.steering_request, {"turnId": conversation.turn_id})
            conversation.steering_request = None
        return None

    def record_input(self, conversation, p):
        body = p["input"][0]["text"].split(":\n", 1)[1]
        sender = p["input"][0]["text"].split(" from ", 1)[1].split(":\n", 1)[0]
        append_line(
            self.home / "inputs.jsonl",
            json.dumps(
                {
                    "thread": conversation.thread_id,
                    "body": body,
                    "input": p["input"][0]["text"],
                    "native_input": p["input"],
                    "at": time.time(),
                }
            )
            + "\n",
            max_record_bytes=INPUT_RECORD_LIMIT,
        )
        if body.startswith("Hive branch identity:"):
            assert conversation.meta.get("forked_from"), "branch child must never use thread/start"
            assert f"You are agent {conversation.meta['agent']}," in body
            body = body.split("\n\nNew task:\n", 1)[1]
        conversation.meta.setdefault("history", []).append(body)
        self.save(conversation)
        return body.split("\n\nHive attachment references", 1)[0], sender

    def quota_error(self, conversation, body):
        if (self.scenario.get("quota_once") or body == self.scenario.get("quota_body")) and not (
            self.home / "quota-seen"
        ).exists():
            publish_text(self.home / "quota-seen", "1")
            reset = self.scenario.get(
                "quota_until", int(time.time()) + self.scenario.get("quota_seconds", 3600)
            )
            if self.scenario.get("quota_error", "usageLimitExceeded") == "usageLimitExceeded":
                self.emit(
                    {
                        "method": "account/rateLimits/updated",
                        "params": {
                            "rateLimits": {
                                "primary": {"usedPercent": 100, "resetsAt": reset},
                                "rateLimitReachedType": "rate_limit_reached",
                            }
                        },
                    }
                )
            self.emit(
                {
                    "method": "error",
                    "params": {
                        "threadId": conversation.thread_id,
                        "turnId": conversation.turn_id,
                        "willRetry": False,
                        "error": {
                            "codexErrorInfo": self.scenario.get("quota_error", "usageLimitExceeded")
                        },
                    },
                }
            )
            if self.scenario.get("quota_disconnect"):
                sys.exit(0)
            self.finish(conversation, "failed")
            return True
        return False
