"""Tool-result continuations on the shared Codex process runtime."""

import json


class Replies:
    def handle_tool_result(self, request, conversation):
        result = request["result"]
        if conversation.meta.pop("tool_reply", False):
            conversation.pending = None
            text = result["contentItems"][0]["text"]
            self.body_reply(
                conversation, json.dumps({"success": result["success"], "text": text}), None
            )
            return
        if conversation.meta.get("comb_script"):
            conversation.meta["comb_results"].append(json.loads(result["contentItems"][0]["text"]))
            self.comb_next(conversation)
            return
        if conversation.pending == "hive_status" and conversation.steering_request is not None:
            assert result["success"], result
            self.reply(conversation.steering_request, {"turnId": conversation.turn_id})
            conversation.steering_request = None
            conversation.pending = None
            self.finish(conversation)
            return
        if conversation.meta.get("ping_pong"):
            self.ping_pong_result(conversation, result)
            return
        if conversation.pending == "hive_send" and conversation.pending_args.get("delay_seconds"):
            assert result["success"], result
        if conversation.pending == "hive_batch":
            assert result["success"], result
        if conversation.pending == "hive_ask":
            assert result["success"], result
            self.emit(
                {
                    "method": "item/completed",
                    "params": {
                        "threadId": conversation.thread_id,
                        "turnId": conversation.turn_id,
                        "item": {
                            "type": "dynamicToolCall",
                            "tool": conversation.pending,
                            "arguments": conversation.pending_args,
                            "status": "completed",
                            "success": True,
                            "contentItems": result["contentItems"],
                        },
                    },
                }
            )
        if conversation.pending == "hive_spawn":
            conversation.meta["reviewer"] = json.loads(result["contentItems"][0]["text"])[
                "agent_id"
            ]
            self.save(conversation)
        if conversation.pending != "input_required" and not (
            conversation.pending == "hive_send" and conversation.pending_args.get("mode") == "steer"
        ):
            self.finish(conversation)
        conversation.pending = None

    def ping_pong_result(self, conversation, result):
        assert result["success"], result
        value = json.loads(result["contentItems"][0]["text"])
        if conversation.pending == "hive_spawn":
            conversation.meta.setdefault("children", []).append(value["agent_id"])
            self.save(conversation)
        self.emit(
            {
                "method": "item/completed",
                "params": {
                    "threadId": conversation.thread_id,
                    "turnId": conversation.turn_id,
                    "item": {
                        "type": "dynamicToolCall",
                        "tool": conversation.pending,
                        "arguments": conversation.pending_args,
                        "status": "completed",
                        "success": True,
                        "contentItems": result["contentItems"],
                    },
                },
            }
        )
        conversation.pending = None
        if conversation.actions:
            self.call(conversation, *conversation.actions.pop(0))
        else:
            self.finish(conversation)
