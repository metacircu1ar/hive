"""Codex fixture bodies on its single protocol runtime."""

import json
import subprocess
import sys
from mock_common import (
    run_stdio_mcp,
    publish_text,
)


class Bodies:
    def body_branch_self(self, conversation, body, sender):
        self.call(
            conversation,
            "hive_spawn",
            {
                "from_agent": conversation.meta["agent"],
                "body": "branch recall",
                "request_id": "branch-self",
            },
        )

    def body_branch_recall(self, conversation, body, sender):
        self.emit(
            {
                "method": "item/completed",
                "params": {
                    "threadId": conversation.thread_id,
                    "turnId": conversation.turn_id,
                    "item": {
                        "type": "agentMessage",
                        "text": json.dumps(
                            {
                                "history": conversation.meta["history"],
                                "agent": conversation.meta["agent"],
                            }
                        ),
                        "phase": "final_answer",
                    },
                },
            }
        )
        self.finish(conversation)

    def body_ask(self, conversation, body, sender):
        self.call(conversation, "hive_ask", json.loads(body[4:]))

    def body_spawn_bound_child(self, conversation, body, sender):
        self.call(
            conversation,
            "hive_spawn",
            {
                "role": "ping",
                "lifetime": "with_parent",
                "body": "hold",
                "request_id": "bound-child",
            },
        )

    def body_spawn_mixed_children(self, conversation, body, sender):
        conversation.meta["ping_pong"] = True
        conversation.actions = [
            ("hive_spawn", {"role": role, "body": "idle", "request_id": role})
            for role in ("reviewer", "ping")
        ]
        self.call(conversation, *conversation.actions.pop(0))

    def body_ping_pong(self, conversation, body, sender):
        conversation.meta["ping_pong"] = True
        conversation.actions = [
            (
                "hive_spawn",
                {
                    "role": self.scenario.get("ping_role", "ping"),
                    "body": "Send ping to your parent now.",
                    "request_id": f"ping-{i:02}",
                },
            )
            for i in range(1, self.scenario.get("ping_count", 10) + 1)
        ]
        self.call(conversation, *conversation.actions.pop(0))

    def body_initial_ping(self, conversation, body, sender):
        conversation.meta["ping_pong"] = True
        self.call(
            conversation,
            "hive_send",
            {"target": conversation.meta["parent"], "body": "ping", "request_id": "initial-ping"},
        )

    def body_ping(self, conversation, body, sender):
        self.call(
            conversation,
            "hive_send",
            {"target": sender, "body": "pong", "request_id": "pong-" + sender},
        )

    def body_pong(self, conversation, body, sender):
        self.call(
            conversation,
            "hive_send",
            {
                "target": conversation.meta["parent"],
                "body": "received pong",
                "request_id": "pong-receipt",
            },
        )

    def body_pong_receipt(self, conversation, body, sender):
        conversation.meta.setdefault("receipts", []).append(sender)
        self.save(conversation)
        if len(set(conversation.meta["receipts"])) == self.scenario.get("ping_count", 10):
            conversation.actions = [
                ("hive_terminate", {"target": child, "request_id": "terminate-" + child})
                for child in conversation.meta["children"]
            ]
            conversation.actions.append(
                (
                    "hive_complete",
                    {"body": "10 pings, pongs and receipts", "request_id": "ping-pong-complete"},
                )
            )
            self.call(conversation, *conversation.actions.pop(0))
        else:
            self.finish(conversation)

    def body_delegate(self, conversation, body, sender):
        self.call(
            conversation,
            "hive_spawn",
            {"role": "reviewer", "body": "review one", "request_id": "spawn-reviewer"},
        )

    def body_delay_self(self, conversation, body, sender):
        self.call(
            conversation,
            "hive_send",
            {
                "target": conversation.meta["agent"],
                "body": "reply:reminder",
                "delay_seconds": 2,
                "request_id": "self-reminder",
            },
        )

    def body_review_one(self, conversation, body, sender):
        self.call(
            conversation,
            "hive_send",
            {"target": conversation.meta["parent"], "body": "findings", "request_id": "findings"},
        )

    def body_findings(self, conversation, body, sender):
        self.call(
            conversation,
            "hive_send",
            {
                "target": conversation.meta["reviewer"],
                "body": "review two",
                "request_id": "second-review",
            },
        )

    def body_review_two(self, conversation, body, sender):
        self.call(
            conversation,
            "hive_send",
            {"target": conversation.meta["parent"], "body": "NO_FINDINGS", "request_id": "clean"},
        )

    def body_complete(self, conversation, body, sender):
        self.call(
            conversation,
            "hive_complete",
            {"body": "reviewed and tested", "request_id": "complete"},
        )

    def body_bad_arguments(self, conversation, body, sender):
        self.call(conversation, "hive_agents", [])

    def body_extra_arguments(self, conversation, body, sender):
        self.call(
            conversation,
            "hive_complete",
            {"body": "wrong", "request_id": "wrong", "target": "someone-else"},
        )

    def body_hold(self, conversation, body, sender):
        pass

    def body_shell(self, conversation, body, sender):
        conversation.command_child = subprocess.Popen(
            ["/bin/bash", "-c", "exec sleep 60"], start_new_session=True
        )
        publish_text(self.home / "shell-child", str(conversation.command_child.pid))
        publish_text(
            self.home / ("shell-child-" + conversation.meta["agent"]),
            str(conversation.command_child.pid),
        )
        if body == "shell background":
            self.finish(conversation)

    def body_steer_self(self, conversation, body, sender):
        self.call(
            conversation,
            "hive_send",
            {
                "target": conversation.meta["agent"],
                "body": "self guidance",
                "mode": "steer",
                "request_id": "self-steer",
            },
        )

    def body_input_required(self, conversation, body, sender):
        conversation.command_child = subprocess.Popen(
            [
                sys.executable,
                "-c",
                "import pathlib,sys,time;time.sleep(3);"
                'pathlib.Path(sys.argv[1]).write_text("survived")',
                self.scenario["survivor"],
            ],
            start_new_session=True,
        )
        conversation.pending = "input_required"
        callback = "unsupported-" + conversation.turn_id
        self.callbacks[callback] = conversation.thread_id
        self.emit(
            {
                "id": callback,
                "method": "item/tool/requestUserInput",
                "params": {"threadId": conversation.thread_id, "turnId": conversation.turn_id},
            }
        )

    def body_reply(self, conversation, body, sender):
        item = {"type": "agentMessage", "text": body, "phase": "final_answer"}
        if self.scenario.get("live_deltas"):
            item["id"] = "msg-" + conversation.turn_id
            self.stream_live(conversation, item["id"], body)
        self.emit(
            {
                "method": "item/completed",
                "params": {
                    "threadId": conversation.thread_id,
                    "turnId": conversation.turn_id,
                    "item": item,
                },
            }
        )
        self.finish(conversation)

    def body_fail(self, conversation, body, sender):
        self.finish(conversation, "failed")

    def comb_next(self, conversation):
        while conversation.actions:
            action = conversation.actions.pop(0)
            if action.get("catalog"):
                conversation.meta["comb_results"].append({"tools": conversation.meta["tools"]})
                continue
            sequence = len(conversation.meta["comb_results"])
            self.call(
                conversation,
                action["name"],
                action.get("arguments", {}),
                action.get("native_id", sequence),
            )
            return
        conversation.meta["comb_script"] = False
        self.body_reply(
            conversation, "comb-results:" + json.dumps(conversation.meta["comb_results"]), "human"
        )

    def handle_body(self, conversation, body, sender):
        routes = (
            (lambda: body.startswith("mcp:"), self.body_mcp),
            (lambda: body.startswith("comb:"), self.body_comb),
            (lambda: body.startswith("joined:"), self.body_joined),
            (lambda: body.startswith("tool:"), self.body_tool),
            (lambda: body.startswith("join-output:"), self.body_join_output),
            (lambda: body == "branch self", self.body_branch_self),
            (lambda: body == "branch recall", self.body_branch_recall),
            (lambda: body.startswith("ask:"), self.body_ask),
            (lambda: body == "spawn bound child", self.body_spawn_bound_child),
            (lambda: body == "spawn mixed children", self.body_spawn_mixed_children),
            (lambda: body == "ping pong scenario", self.body_ping_pong),
            (lambda: body == "Send ping to your parent now.", self.body_initial_ping),
            (lambda: body == "ping", self.body_ping),
            (lambda: body == "pong", self.body_pong),
            (lambda: body == "received pong", self.body_pong_receipt),
            (lambda: body == "delegate", self.body_delegate),
            (lambda: body == "delay-self", self.body_delay_self),
            (lambda: body == "review one", self.body_review_one),
            (lambda: body == "findings", self.body_findings),
            (lambda: body == "review two", self.body_review_two),
            (lambda: body in ["NO_FINDINGS", "complete"], self.body_complete),
            (lambda: body == "bad arguments", self.body_bad_arguments),
            (lambda: body == "extra arguments", self.body_extra_arguments),
            (lambda: body == "hold", self.body_hold),
            (lambda: body in ("shell hold", "shell background"), self.body_shell),
            (lambda: body == "steer self", self.body_steer_self),
            (lambda: body == "input required", self.body_input_required),
            (lambda: body.startswith(("reply:", "Approval ")), self.body_reply),
            (lambda: body == "fail", self.body_fail),
        )
        for matches, handler in routes:
            if matches():
                handler(conversation, body, sender)
                return
        self.finish(conversation)

    def body_mcp(self, conversation, body, sender):
        self.body_reply(
            conversation, run_stdio_mcp(conversation.meta["mcp_servers"][body[4:]]), sender
        )

    def body_comb(self, conversation, body, sender):
        conversation.meta["comb_script"] = True
        conversation.meta["comb_results"] = []
        conversation.actions = json.loads(body[len("comb:") :])
        self.comb_next(conversation)

    def body_joined(self, conversation, body, sender):
        self.call(conversation, "hive_batch", json.loads(body[7:]))

    def body_tool(self, conversation, body, sender):
        request = json.loads(body[5:])
        conversation.meta["tool_reply"] = True
        self.call(conversation, request["name"], request["arguments"])

    def body_join_output(self, conversation, body, sender):
        character = {"ascii": "x", "control": "\x01", "utf8": "🙂"}[body.split(":")[1]]
        self.body_reply(conversation, character * (65536 // len(character.encode())), sender)
