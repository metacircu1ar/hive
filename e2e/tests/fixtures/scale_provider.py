#!/usr/bin/env python3
"""Deterministic, multiplexed app-server fixture. Never invokes a model or network.

Conversation state belongs to this fixture, not the runtime database. Historical
metadata stays until process exit; the loaded-thread inventory reflects unsubscribe.
The append journal survives recycling
without rewriting a growing coordinator snapshot for every receipt.
"""

import collections
import json
import os
import re
import select
import sqlite3
import sys
import time
import uuid
from pathlib import Path

from mock_common import (
    schema_probe,
    decode_scenario,
    publish_json,
    append_jsonl,
    CompleteLines,
    AppendStream,
)


def require(value, message):
    if not value:
        raise RuntimeError(message)


SPAWN_RETRY_LIMIT = 16
SPAWN_RETRY_ERROR = {
    "error": {
        "code": "agent_busy",
        "message": "Catalog discovery capacity is occupied; retry.",
    }
}


def catalog_spawn_busy(name, result):
    if name != "hive_spawn" or result.get("success") is not False:
        return False
    items = result.get("contentItems")
    if (
        not isinstance(items, list)
        or len(items) != 1
        or not isinstance(items[0], dict)
        or items[0].get("type") != "inputText"
        or not isinstance(items[0].get("text"), str)
    ):
        return False
    return json.loads(items[0]["text"]) == SPAWN_RETRY_ERROR


class Provider:
    def __init__(self):
        self.home = Path(os.environ["CODEX_HOME"]) / "scale-mock"
        self.home.mkdir(exist_ok=True)
        self.identity = f"{os.getpid()}-{uuid.uuid4()}"
        self.trace = AppendStream.create_new(
            self.home / f"trace-{self.identity}.jsonl", durable=False
        ).__enter__()
        self.threads = {}
        self.loaded_threads = set()
        self.turns = {}
        self.pending = {}
        self.barrier = []
        self.barrier_deadline = None
        self.barrier_done = False
        self.pair_db = None
        self.connection_logged = False
        self.experimental = False
        # Opt-in streamed text per turn, sent only to processes that did not opt out.
        options = Path(__file__).with_name("scale-scenario.json")
        self.deltas = json.loads(options.read_text()).get("deltas", 0) if options.exists() else 0
        self.opted_out = set()
        selected = [
            arg.split("=", 1)[1]
            for arg in sys.argv
            if arg.startswith("features.enable_request_compression=")
        ]
        require(
            len(selected) <= 1 and all(value in ("true", "false") for value in selected),
            "Invalid process setting",
        )
        self.compression = selected[0] if selected else "default"
        self.log("process_start", ppid=os.getppid(), compression=self.compression)

    def runtime_connection(self):
        require(self.experimental, "Runtime request without experimental capabilities")
        if not self.connection_logged:
            self.log("connection", runtime=True)
            self.connection_logged = True

    def log(self, kind, **values):
        self.trace.append_jsonl(
            dict(
                kind=kind,
                ns=time.monotonic_ns(),
                pid=os.getpid(),
                process=self.identity,
                **values,
            )
        )

    def emit(self, value):
        print(json.dumps(value, separators=(",", ":")), flush=True)

    def reply(self, request, result):
        self.emit({"id": request["id"], "result": result})

    def journal(self, thread, change):
        meta = self.threads[thread]
        if meta["role"].startswith("pair-"):
            require(set(change) == {"children"} and not meta["children"], "Invalid pair journal")
            self.pair_db.execute(
                "UPDATE conversations SET child=? WHERE thread=?", (change["children"][0], thread)
            )
            self.pair_db.commit()
        else:
            append_jsonl(self.home / f"{thread}.jsonl", change)
        meta["children"].extend(change.get("children", []))
        meta["child_set"].update(change.get("children", []))
        if "receipt" in change:
            meta["receipts"].add(change["receipt"])
        if "count" in change:
            meta["count"] = change["count"]

    def bind(self, request):
        p = request["params"]
        identity = re.search(
            r"You are Hive agent ([^,]+), role ([^,]+), parent ([^.]+)\.",
            p["developerInstructions"],
        )
        require(identity, "Missing runtime identity")
        agent, role, parent = identity.groups()
        require(
            p["approvalPolicy"] == "never" and p["sandbox"] == "read-only", "Wrong execution policy"
        )
        require(role in ("pong", "ping", "pair-pong", "pair-ping"), "Unexpected role")
        require(
            p["model"] == "gpt-5.6-luna" and p["config"]["model_reasoning_effort"] == "low",
            "Wrong model/effort",
        )
        thread = p.get("threadId", "mock-" + agent)
        path = self.home / f"{thread}.json"
        resumed = request["method"] == "thread/resume"
        pair = role.startswith("pair-")
        if pair and self.pair_db is None:
            # Independent fixture persistence, never the Hive runtime DB.
            # One indexed store avoids a million per-conversation files.
            self.open_pair_store()
        if resumed:
            require(p.get("excludeTurns") is True and "dynamicTools" not in p, "Invalid resume")
            stored = (
                self.pair_db.execute(
                    "SELECT meta,child FROM conversations WHERE thread=?", (thread,)
                ).fetchone()
                if pair
                else None
            )
            meta = json.loads(stored[0] if pair else path.read_text())
            require(
                meta["agent"] == agent
                and meta["role"] == role
                and meta["parent"] == parent
                and meta["cwd"] == p["cwd"]
                and meta["model"] == p["model"]
                and meta["tools"],
                "Resume lost identity, tools, or configuration",
            )
        else:
            require(pair or not path.exists(), "Conversation was created twice")
            meta = dict(
                agent=agent,
                role=role,
                parent=parent,
                cwd=p["cwd"],
                model=p["model"],
                tools=[{"name": t["name"]} for t in p["dynamicTools"]],
            )
            if pair:
                self.pair_db.execute(
                    "INSERT INTO conversations(thread,meta) VALUES(?,?)", (thread, json.dumps(meta))
                )
                self.pair_db.commit()
            else:
                publish_json(path, meta, create_once=True)
        meta.update(children=[], receipts=set())
        journal = self.home / f"{thread}.jsonl"
        if pair and resumed and stored[1]:
            meta["children"].append(stored[1])
        if not pair and journal.exists():
            self.replay_journal(meta, journal)
        meta["child_set"] = set(meta["children"])
        self.threads[thread] = meta
        self.loaded_threads.add(thread)
        self.log("thread_resume" if resumed else "thread_start", thread=thread, agent=agent)
        self.reply(
            request,
            {
                "thread": {"id": thread},
                "model": p["model"],
                "reasoningEffort": "low",
                "sandbox": {"type": "readOnly"},
            },
        )

    def finish(self, thread, status="completed"):
        turn = self.turns.pop(thread)
        if self.deltas and "item/agentMessage/delta" not in self.opted_out:
            for index in range(self.deltas):
                self.emit(
                    {
                        "method": "item/agentMessage/delta",
                        "params": {
                            "threadId": thread,
                            "turnId": turn["id"],
                            "itemId": "msg-" + turn["id"],
                            "delta": f"word{index} ",
                        },
                    }
                )
        self.log("turn_end", thread=thread, turn=turn["id"], status=status)
        self.emit(
            {
                "method": "turn/completed",
                "params": {"threadId": thread, "turn": {"id": turn["id"], "status": status}},
            }
        )

    def advance(self, thread):
        turn = self.turns.get(thread)
        if not turn or "retry_at" in turn:
            return
        if not turn["actions"]:
            self.finish(thread)
            return
        name, args = turn["actions"].popleft()
        require(
            any(t["name"] == name for t in self.threads[thread]["tools"]), "Tool was not advertised"
        )
        turn["action"] += 1
        rid = f"{turn['id']}/{turn['action']}"
        self.pending[rid] = (thread, turn["id"], name, args)
        self.emit(
            {
                "id": rid,
                "method": "item/tool/call",
                "params": {
                    "threadId": thread,
                    "turnId": turn["id"],
                    "callId": rid,
                    "tool": name,
                    "arguments": args,
                },
            }
        )

    @staticmethod
    def batches(operations, prefix):
        for offset in range(0, len(operations), 64):
            yield (
                "hive_batch",
                {
                    "request_id": f"{prefix}-{offset}",
                    "operations": operations[offset : offset + 64],
                },
            )

    def start_turn(self, request):
        p = request["params"]
        thread = p["threadId"]
        require(thread not in self.turns, "Overlapping turns in one conversation")
        meta = self.threads[thread]
        require(p["model"] == meta["model"] and p["effort"] == "low", "Turn changed model/effort")
        header, body = p["input"][0]["text"].split(":\n", 1)
        message, sender = re.fullmatch(r"Hive message (\S+) from (\S+)", header).groups()
        turn_id = f"{thread}/{message}"
        actions = (
            self.pair_actions(meta, body, sender)
            if meta["role"].startswith("pair-")
            else self.coordinator_actions(thread, meta, body, sender)
        )
        self.turns[thread] = dict(id=turn_id, actions=collections.deque(actions), action=0)
        self.log("turn_start", thread=thread, turn=turn_id, message=message)
        self.reply(request, {"turn": {"id": turn_id}})
        self.emit(
            {
                "method": "turn/started",
                "params": {"threadId": thread, "turn": {"id": turn_id, "status": "inProgress"}},
            }
        )
        # On each shard, prove two distinct ping turns overlap before either is
        # allowed to emit its ping. A singleton final cohort uses a timed release;
        # only an actual two-thread release counts as barrier evidence.
        if (
            body in ("Send ping to your parent now.", "Start one ping pong pair.")
            and not self.barrier_done
        ):
            self.barrier.append(thread)
            if self.barrier_deadline is None:
                self.barrier_deadline = time.monotonic() + 0.2
            if len(self.barrier) >= 2:
                self.release_barrier(True)
        else:
            self.advance(thread)

    def release_barrier(self, overlapped):
        held, self.barrier = self.barrier, []
        self.barrier_done = True
        self.barrier_deadline = None
        self.log("barrier", threads=held, overlapped=overlapped)
        for thread in held:
            self.advance(thread)

    def handle(self, request):
        method, p = request.get("method"), request.get("params", {})
        if method is None:
            self.handle_tool_result(request)
        elif method == "initialize":
            # Catalog discovery shares experimental initialization but never opens a session.
            self.experimental = bool(p.get("capabilities", {}).get("experimentalApi"))
            self.opted_out = set(p.get("capabilities", {}).get("optOutNotificationMethods") or [])
            self.reply(request, {})
        elif method == "initialized":
            pass
        elif method == "account/read":
            self.reply(request, {"requiresOpenaiAuth": True, "account": {"type": "chatgpt"}})
        elif method == "account/rateLimits/read":
            self.runtime_connection()
            self.reply(request, {"rateLimits": {}, "ordinaryUsageAllowed": True})
        elif method == "model/list":
            self.reply(
                request,
                {
                    "data": [
                        {
                            "model": "gpt-5.6-luna",
                            "supportedReasoningEfforts": [{"reasoningEffort": "low"}],
                        }
                    ],
                    "nextCursor": None,
                },
            )
        elif method in ("thread/start", "thread/resume"):
            self.runtime_connection()
            self.bind(request)
        elif method == "turn/start":
            self.start_turn(request)
        elif method == "turn/steer":
            # Scale ping-pong covers next-turn traffic. Explicitly reject the
            # session capability probe without inventing steering support.
            self.emit(
                {
                    "id": request["id"],
                    "error": {
                        "code": -32601,
                        "message": "Steering is not implemented by this fixture",
                    },
                }
            )
        elif method == "turn/interrupt":
            self.reply(request, {})
            if p["threadId"] in self.turns:
                require(self.turns[p["threadId"]]["id"] == p["turnId"], "Wrong interrupt target")
                self.finish(p["threadId"], "interrupted")
        elif method == "thread/backgroundTerminals/clean":
            require(p["threadId"] not in self.turns, "Cleanup before turn completion")
            self.reply(request, {})
        elif method == "thread/backgroundTerminals/list":
            self.reply(request, {"data": [], "nextCursor": None})
        elif method == "thread/unsubscribe":
            require(p["threadId"] not in self.turns, "Unsubscribe before completion")
            self.log("unsubscribe", thread=p["threadId"])
            self.loaded_threads.discard(p["threadId"])
            self.reply(request, {"status": "unsubscribed"})
        elif method == "thread/loaded/list":
            self.reply(request, {"data": sorted(self.loaded_threads), "nextCursor": None})
        else:
            raise RuntimeError(f"Unexpected request: {method}")

    def run(self):
        pending = b""
        while True:
            timeout = self.next_timeout()
            readable, _, _ = select.select([sys.stdin.fileno()], [], [], timeout)
            if readable:
                data = os.read(sys.stdin.fileno(), 65536)
                if not data:
                    require(not pending and not self.turns, "EOF with unfinished input or turns")
                    if not self.connection_logged:
                        self.log("connection", runtime=False)
                    self.log("process_exit")
                    if self.pair_db is not None:
                        self.pair_db.close()
                    self.trace.close()
                    return
                pending += data
                while b"\n" in pending:
                    line, pending = pending.split(b"\n", 1)
                    self.handle(json.loads(line))
            if self.barrier_deadline is not None and time.monotonic() >= self.barrier_deadline:
                self.release_barrier(False)
            self.advance_retries()

    def next_timeout(self):
        deadlines = [turn["retry_at"] for turn in self.turns.values() if "retry_at" in turn]
        if self.barrier_deadline is not None:
            deadlines.append(self.barrier_deadline)
        return max(0, min(deadlines) - time.monotonic()) if deadlines else None

    def advance_retries(self):
        now = time.monotonic()
        for thread, turn in list(self.turns.items()):
            if "retry_at" in turn and now >= turn["retry_at"]:
                del turn["retry_at"]
                self.advance(thread)

    def retry_spawn(self, thread, name, args):
        turn = self.turns[thread]
        attempt = turn.get("spawn_retries", 0) + 1
        require(attempt <= SPAWN_RETRY_LIMIT, "Catalog spawn retry limit exceeded")
        turn["spawn_retries"] = attempt
        turn["retry_at"] = time.monotonic() + min(0.05 * 2 ** (attempt - 1), 1)
        turn["actions"].appendleft((name, args))

    def open_pair_store(self):
        self.pair_db = sqlite3.connect(self.home / "pairs.sqlite", timeout=30)
        self.pair_db.execute("PRAGMA journal_mode=WAL")
        self.pair_db.execute("PRAGMA synchronous=NORMAL")
        self.pair_db.execute("PRAGMA cache_size=-2048")
        self.pair_db.execute(
            "CREATE TABLE IF NOT EXISTS conversations(thread TEXT PRIMARY KEY, meta TEXT NOT NULL, child TEXT)"
        )

    def handle_tool_result(self, request):
        thread, expected_turn, name, args = self.pending.pop(request["id"])
        if thread not in self.turns or self.turns[thread]["id"] != expected_turn:
            return  # Reply raced an acknowledged interrupt.
        result = request.get("result", {})
        if catalog_spawn_busy(name, result):
            self.tool_completed(thread, name, args, result)
            self.retry_spawn(thread, name, args)
            return
        require(result.get("success"), f"Tool failed: {request}")
        self.turns[thread].pop("spawn_retries", None)
        data = json.loads(result["contentItems"][0]["text"])
        if name == "hive_batch" and args["operations"][0]["op"] == "spawn":
            self.journal(thread, {"children": [r["agent_id"] for r in data["results"]]})
        if name == "hive_spawn":
            self.journal(thread, {"children": [data["agent_id"]]})
            self.turns[thread]["actions"].append(
                (
                    "hive_send",
                    {"target": data["agent_id"], "body": "ping", "request_id": "send-ping"},
                )
            )
        self.tool_completed(thread, name, args, result)
        self.advance(thread)

    def tool_completed(self, thread, name, args, result):
        self.emit(
            {
                "method": "item/completed",
                "params": {
                    "threadId": thread,
                    "turnId": self.turns[thread]["id"],
                    "item": {
                        "type": "dynamicToolCall",
                        "tool": name,
                        "arguments": args,
                        "status": "completed",
                        "success": result["success"],
                        "contentItems": result["contentItems"],
                    },
                },
            }
        )

    def pair_actions(self, meta, body, sender):
        actions = []
        if body == "Start one ping pong pair.":
            require(
                sender == "human"
                and meta["role"] == "pair-ping"
                and meta["parent"] == "none"
                and not meta["children"],
                "Invalid pair seed",
            )
            actions = [
                (
                    "hive_spawn",
                    {
                        "role": "pair-pong",
                        "body": "Wait for ping from your parent.",
                        "request_id": "spawn-pong",
                    },
                )
            ]
        elif body == "Wait for ping from your parent.":
            require(
                meta["role"] == "pair-pong" and sender == meta["parent"], "Invalid pong startup"
            )
        elif body == "ping":
            require(
                meta["role"] == "pair-pong" and sender == meta["parent"],
                "Wrong pair ping sender",
            )
            actions = [
                (
                    "hive_send",
                    {"target": sender, "body": "pong", "request_id": "reply-pong"},
                )
            ]
        elif body == "pong":
            require(
                meta["role"] == "pair-ping" and meta["children"] == [sender],
                "Wrong pair pong sender",
            )
            actions = [
                ("hive_terminate", {"target": sender, "request_id": "stop-pong"}),
                (
                    "hive_complete",
                    {"body": "Pair completed", "request_id": "pair-complete"},
                ),
            ]
        else:
            raise RuntimeError("Unexpected pair input")
        return actions

    def coordinator_actions(self, thread, meta, body, sender):
        actions = []
        if sender == "human":
            count = decode_scenario(body)["children"]
            require(meta["role"] == "pong" and not meta["children"], "Invalid initial task")
            self.journal(thread, {"count": count})
            actions = list(
                self.batches(
                    [
                        {
                            "op": "spawn",
                            "role": "ping",
                            "body": "Send ping to your parent now.",
                            "request_id": f"ping-{i}",
                        }
                        for i in range(count)
                    ],
                    "spawn",
                )
            )
        elif body == "Send ping to your parent now.":
            require(meta["role"] == "ping" and sender == meta["parent"], "Invalid startup")
            actions = [
                ("hive_send", {"target": sender, "body": "ping", "request_id": "initial-ping"})
            ]
        elif body == "ping":
            require(meta["role"] == "pong" and sender in meta["child_set"], "Unknown child")
            actions = [
                (
                    "hive_send",
                    {"target": sender, "body": "pong", "request_id": "pong-" + sender},
                )
            ]
        elif body == "pong":
            require(meta["role"] == "ping" and sender == meta["parent"], "Wrong pong sender")
            actions = [
                (
                    "hive_send",
                    {"target": sender, "body": "received pong", "request_id": "pong-receipt"},
                )
            ]
        elif body == "received pong":
            require(
                meta["role"] == "pong"
                and sender in meta["child_set"]
                and sender not in meta["receipts"],
                "Invalid or duplicate receipt",
            )
            self.journal(thread, {"receipt": sender})
            if len(meta["receipts"]) == meta["count"]:
                actions = list(
                    self.batches(
                        [
                            {"op": "terminate", "target": child, "request_id": "terminate-" + child}
                            for child in meta["children"]
                        ],
                        "stop",
                    )
                )
                actions.append(
                    (
                        "hive_complete",
                        {
                            "body": f"{meta['count']} exchanges completed",
                            "request_id": "ping-pong-complete",
                        },
                    )
                )
        else:
            raise RuntimeError("Unexpected input")
        return actions

    def replay_journal(self, meta, journal):
        with CompleteLines(journal) as stream:
            for line in stream:
                change = json.loads(line)
                meta["children"].extend(change.get("children", []))
                if "receipt" in change:
                    meta["receipts"].add(change["receipt"])
                if "count" in change:
                    meta["count"] = change["count"]


if __name__ == "__main__":
    if schema_probe(sys.argv):
        raise SystemExit()
    if sys.argv[-1] == "--version":
        print("codex-cli 0.163.0")
    else:
        require(sys.argv[1:].count("app-server") == 1, "Only mock app-server is supported")
        Provider().run()
