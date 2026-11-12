"""Codex fixture threads on its single protocol runtime."""

import json
import os
import pathlib
import re
import time
import uuid
from mock_common import (
    wait_for_gate,
    publish_text,
    append_line,
    claim_counter,
)


class Threads:
    def handle_thread(self, request, conversation):
        method = request.get("method")
        p = request.get("params", {})
        if method == "thread/fork" and not self.fork_thread(request, conversation, p):
            return
        if method == "thread/resume" and self.scenario.get("reject_resume"):
            self.emit({"id": request["id"], "error": {"code": -1, "message": "resume rejected"}})
            return
        if method == "thread/start":
            conversation.thread_id = str(uuid.uuid4())
            parent = re.search(r"parent ([^. ]+)", p["developerInstructions"]).group(1)
            agent = re.search(r"You are Hive agent ([^,]+)", p["developerInstructions"]).group(1)
            conversation.meta = {
                "instructions": p["developerInstructions"],
                "parent": parent,
                "agent": agent,
                "model": p["model"],
                "effort": p["config"]["model_reasoning_effort"],
                "tools": p["dynamicTools"],
            }
            self.save(conversation)
        elif method == "thread/resume":
            conversation.thread_id = p["threadId"]
            conversation.meta = json.loads(
                (self.home / (conversation.thread_id + ".json")).read_text()
            )
            assert "dynamicTools" not in p
            assert conversation.meta["tools"]
        conversation.meta["mcp_servers"] = p["config"]["mcp_servers"]
        self.loaded_threads.add(conversation.thread_id)
        selection = {
            "method": method,
            "thread": conversation.thread_id,
            "pid": os.getpid(),
            "model": p["model"],
            "config": p["config"],
        }
        if self.scenario.get("unload_after_unsubscribe_ms") is not None:
            selection["at_ns"] = time.monotonic_ns()
        append_line(self.home / "thread-selections.jsonl", json.dumps(selection) + "\n")
        self.save(conversation)
        retained = method == "thread/resume" and self.scenario.get("resume_ignores_selection")
        if method == "thread/resume" and self.scenario.get("resume_echo_model"):
            conversation.meta["model"] = self.scenario["resume_echo_model"]
        if method == "thread/resume" and self.scenario.get("resume_echo_effort"):
            conversation.meta["effort"] = self.scenario["resume_echo_effort"]
        native = self.native_rollout(conversation)
        self.reply(
            request,
            {
                "thread": {
                    "id": conversation.thread_id,
                    "path": None
                    if self.scenario.get("omit_native_path")
                    else str(native.resolve()),
                },
                "model": conversation.meta["model"] if retained else p["model"],
                "reasoningEffort": (
                    conversation.meta["effort"]
                    if retained
                    else p["config"]["model_reasoning_effort"]
                ),
                "sandbox": {"type": "workspaceWrite"},
            },
        )

    def fork_thread(self, request, conversation, p):
        source = json.loads((self.home / (p["threadId"] + ".json")).read_text())
        assert p["lastTurnId"] == source["last_turn"], (p, source)
        assert p["excludeTurns"] is True and p["ephemeral"] is False
        rejections = self.scenario.get("fork_rejections", [])
        rejected = claim_counter(self.home / "fork-rejected", len(rejections))
        if rejected is not None:
            self.emit(
                {
                    "id": request["id"],
                    "error": {
                        "code": -32600,
                        "message": rejections[rejected].format(turn=p["lastTurnId"]),
                    },
                }
            )
            return False
        conversation.thread_id = str(uuid.uuid4())
        conversation.meta = source
        conversation.meta["forked_from"] = p["threadId"]
        conversation.meta["agent"] = re.search(
            r"You are Hive agent ([^,]+)", p["developerInstructions"]
        ).group(1)
        conversation.meta["parent"] = re.search(
            r"parent ([^. ]+)", p["developerInstructions"]
        ).group(1)
        conversation.meta["instructions"] = p["developerInstructions"]
        conversation.meta["model"], conversation.meta["effort"] = (
            p["model"],
            p["config"]["model_reasoning_effort"],
        )
        self.save(conversation)
        if self.scenario.get("fork_hold_after_creation"):
            publish_text(self.home / "fork-created", conversation.thread_id)
            wait_for_gate(self.home / "fork-release")
        return True

    def native_rollout(self, conversation):
        native = (
            self.home
            / "sessions"
            / "2026"
            / "11"
            / "05"
            / ("rollout-2026-11-05T00-00-00-" + conversation.thread_id + ".jsonl")
        )
        for directory in reversed(native.parent.parents):
            if directory != self.home and self.home not in directory.parents:
                continue
            directory.mkdir(mode=0o700, exist_ok=True)
        native.parent.mkdir(mode=0o700, exist_ok=True)
        if not native.exists():
            publish_text(native, "owned native fixture\n")
        return native

    def handle_clean(self, request, conversation):
        if self.scenario.get("unload_after_unsubscribe_ms") is not None:
            self.refresh_unloads()
            if conversation.thread_id not in self.loaded_threads:
                self.emit({"id": request["id"],
                           "error": {"code": -32600, "message": "thread unloaded"}})
                return
        if self.scenario.get("unexpected_turn"):
            self.emit(
                {
                    "method": "turn/started",
                    "params": {
                        "threadId": conversation.thread_id,
                        "turn": {"id": "unexpected", "status": "inProgress"},
                    },
                }
            )
        if self.scenario.get("cleanup_refused"):
            self.emit({"id": request["id"], "error": {"code": -1, "message": "cleanup refused"}})
        else:
            if conversation.command_child:
                conversation.command_child.terminate()
                conversation.command_child.wait()
                conversation.command_child = None
            self.reply(request, {})

    def handle_unsubscribe(self, request, conversation):
        delay = self.scenario.get("unload_after_unsubscribe_ms")
        if delay is not None:
            self.__dict__.setdefault("pending_unloads", {})[conversation.thread_id] = (
                time.monotonic() + delay / 1000
            )
            publish_text(self.home / ("unsubscribed-" + conversation.thread_id), "pending")
        elif not self.scenario.get("retain_loaded_after_unsubscribe"):
            self.loaded_threads.discard(conversation.thread_id)
        self.reply(request, {"status": "unsubscribed"})

    def refresh_unloads(self):
        pending = self.__dict__.setdefault("pending_unloads", {})
        for thread, deadline in list(pending.items()):
            if time.monotonic() >= deadline:
                self.loaded_threads.discard(thread)
                del pending[thread]
                publish_text(self.home / ("unloaded-" + thread), str(time.monotonic_ns()))
                self.emit({"method": "thread/closed", "params": {"threadId": thread}})

    def handle_loaded_threads(self, request, conversation):
        self.refresh_unloads()
        if self.scenario.get("loaded_list_first_unanswered") and not self.__dict__.get(
            "unanswered_list_seen"
        ):
            self.__dict__["unanswered_list_seen"] = True
            publish_text(self.home / "loaded-list-unanswered", "pending")
        elif self.scenario.get("loaded_list_error"):
            self.emit({"id": request["id"], "error": {"code": -32600, "message": "list refused"}})
        else:
            cursor = "incomplete" if self.scenario.get("loaded_list_cursor") else None
            self.reply(request, {"data": sorted(self.loaded_threads), "nextCursor": cursor})

    def handle_terminals(self, request, conversation):
        if self.scenario.get("unload_after_unsubscribe_ms") is not None:
            self.refresh_unloads()
            if conversation.thread_id not in self.loaded_threads:
                self.emit({"id": request["id"],
                           "error": {"code": -32600, "message": "thread unloaded"}})
                return
        conversation.list_calls += 1
        if self.scenario.get("exiting_terminal") and conversation.list_calls == 1:
            self.reply(request, {"data": [{"id": "exiting"}]})
        elif self.scenario.get("omit_cursor"):
            self.reply(request, {"data": []})
        else:
            self.reply(request, {"data": [], "nextCursor": None})
        if self.scenario.get("list_marker"):
            publish_text(pathlib.Path(self.scenario["list_marker"]), str(conversation.list_calls))
        if self.scenario.get("late_after_list") == conversation.list_calls:
            self.late_terminal(conversation)
            for _ in range(self.scenario.get("late_burst", 0)):
                self.emit(
                    {
                        "method": "item/completed",
                        "params": {
                            "threadId": conversation.thread_id,
                            "turnId": conversation.turn_id,
                            "item": {"id": "late-command", "type": "commandExecution"},
                        },
                    }
                )
        if self.scenario.get("late_continuous"):
            self.emit(
                {
                    "method": "item/completed",
                    "params": {
                        "threadId": conversation.thread_id,
                        "turnId": conversation.turn_id,
                        "item": {"id": "late-telemetry", "type": "commandExecution"},
                    },
                }
            )
