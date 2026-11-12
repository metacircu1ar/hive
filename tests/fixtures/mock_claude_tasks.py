"""Claude fixture tasks on its single protocol runtime."""

import json
import sys
from pathlib import Path
from mock_common import (
    run_stdio_mcp,
    wait_for_gate,
    publish_text,
)


class Tasks:
    def body_recycle(self):
        for number in range(2100):
            self.emit(
                {
                    "type": "stream_event",
                    "session_id": self.session,
                    "event": {
                        "type": "content_block_start",
                        "content_block": {
                            "type": "tool_use",
                            "id": "stream-" + str(number),
                            "name": "Read",
                            "input": {},
                        },
                    },
                }
            )
        if self.scenario().get("recycle_gate"):
            publish_text(self.home / "recycle-ready", "ready")
            wait_for_gate(
                Path(self.scenario()["recycle_gate"]),
                timeout=15,
                timeout_error=RuntimeError("recycle gate timed out"),
            )
        self.complete("recycle-finished")
        return

    def body_backpressure(self):
        assert self.body == "backpressure:" + "x" * 65523
        self.complete("large-input-intact")
        return

    def body_api_error(self):
        self.emit(
            {
                "type": "result",
                "session_id": self.session,
                "subtype": "success",
                "is_error": True,
                "api_error_status": 529,
                "terminal_reason": "completed",
            }
        )
        self.turn = None
        # Native runHeadless exits1 after a terminal is_error result on EOF.
        if self.body == "api-error":
            sys.stdin.read()
        raise SystemExit(1)

    def body_hold_mode(self):
        self.mode = self.body.removeprefix("hold-")
        return

    def body_hold(self):
        return

    def body_split_reply(self):
        fragments = (
            ["x" * 4096] * 8 if self.body.endswith("-large") else ["first block", "second block"]
        )
        for text in fragments:
            self.emit(
                {
                    "type": "assistant",
                    "session_id": self.session,
                    "message": {
                        "id": "native-" + self.turn,
                        "model": self.native_model,
                        "content": [{"type": "text", "text": text}],
                    },
                }
            )
        self.finish()
        return

    def body_mcp(self):
        name = self.body[4:]
        assert f"mcp__{name}__*" in self.option("--allowedTools")
        config = json.loads(Path(self.option("--mcp-config")).read_text())
        self.complete(run_stdio_mcp(config["mcpServers"][name]))

    def body_comb(self):
        self.comb_actions = json.loads(self.body[len("comb:") :])
        self.comb_results = []
        self.comb_next()

    def body_joined(self):
        self.tool("hive_batch", json.loads(self.body[7:]), "asked")

    def body_join_output(self):
        character = {"ascii": "x", "control": "\x01", "utf8": "🙂"}[self.body.split(":")[1]]
        self.complete(character * (65536 // len(character.encode())))

    def body_joined_reply(self):
        self.complete("joined reply consumed")

    def body_branch_self(self):
        self.tool(
            "hive_spawn",
            {"from_agent": self.agent, "body": "branch recall", "request_id": "branch-self"},
            "spawned",
        )

    def body_branch_recall(self):
        self.complete(json.dumps({"history": self.history, "agent": self.agent}))

    def body_ask(self):
        self.tool("hive_ask", json.loads(self.body[4:]), "asked")

    def body_delay(self):
        self.tool(
            "hive_send",
            {
                "target": self.agent,
                "body": "reply:reminder",
                "delay_seconds": 2,
                "request_id": "self-reminder",
            },
            "scheduled",
        )

    def body_bound(self):
        self.tool(
            "hive_spawn",
            {
                "role": "claude",
                "lifetime": "with_parent",
                "body": "hold",
                "request_id": "bound-child",
            },
            "spawned",
        )

    def body_reviewer(self):
        self.tool(
            "hive_spawn",
            {"role": "claude-reviewer", "body": "review-one", "request_id": "spawn-" + self.agent},
            "spawned",
        )

    def body_review(self):
        parent = (
            Path(self.option("--append-system-prompt-file"))
            .read_text()
            .split("parent ", 1)[1]
            .split(".", 1)[0]
        )
        self.tool(
            "hive_send",
            {"target": parent, "body": "review-ok", "request_id": "review-" + self.agent},
            "sent",
        )

    def body_review_ok(self):
        self.tool(
            "hive_complete", {"body": "reviewed", "request_id": "complete-" + self.agent}, "sent"
        )

    def dispatch_body(self):
        routes = (
            (lambda: self.body == "native:recycle", self.body_recycle),
            (lambda: self.body in ("native:agent", "native:agent-hold"), self.body_native_agent),
            (lambda: self.body == "skill:fork", self.body_skill),
            (
                lambda: (
                    self.body
                    in (
                        "shell-guard",
                        "shell-sigchld",
                        "shell-stray",
                        "shell-killed",
                        "shell-interrupt",
                        "shell-retained",
                    )
                ),
                self.body_shell,
            ),
            (lambda: self.body in ("git-status", "git-denied"), self.body_git),
            (lambda: self.body.startswith("backpressure:"), self.body_backpressure),
            (lambda: self.body in ("api-error", "api-error-exit"), self.body_api_error),
            (
                lambda: self.body in ("hold-error-interrupt", "hold-error-interrupt-exit"),
                self.body_hold_mode,
            ),
            (lambda: self.body == "hold", self.body_hold),
            (lambda: self.body in ("reply:split", "reply:split-large"), self.body_split_reply),
            (lambda: self.body.startswith("mcp:"), self.body_mcp),
            (lambda: self.body.startswith("comb:"), self.body_comb),
            (lambda: self.body.startswith("joined:"), self.body_joined),
            (lambda: self.body.startswith("join-output:"), self.body_join_output),
            (lambda: self.body.startswith("Joined results for "), self.body_joined_reply),
            (lambda: self.body == "branch self", self.body_branch_self),
            (lambda: self.body == "branch recall", self.body_branch_recall),
            (lambda: self.body.startswith("ask:"), self.body_ask),
            (lambda: self.body == "delay-self", self.body_delay),
            (lambda: self.body == "spawn bound child", self.body_bound),
            (lambda: self.body == "spawn-reviewer", self.body_reviewer),
            (lambda: self.body == "review-one", self.body_review),
            (lambda: self.body == "review-ok", self.body_review_ok),
        )
        for matches, handler in routes:
            if matches():
                handler()
                return
        self.complete("answer:" + self.body)
        return
