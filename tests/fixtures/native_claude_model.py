"""Scripted local Messages endpoint, never an account or inference service.

Claude itself creates the native agents, executes tools and emits their frames.
The endpoint only supplies deterministic assistant/tool requests. Observation
receipts contain roles, phases and offered tool names, never request bodies.
"""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import shlex
import sys
import threading
import time
import uuid

from mock_common import append_line, publish_text


def stdio_docs():
    for line in sys.stdin:
        request = json.loads(line)
        if "id" not in request:
            continue
        method = request["method"]
        if method == "initialize":
            result = {
                "protocolVersion": request["params"]["protocolVersion"],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "native-docs", "version": "1"},
            }
        elif method == "tools/list":
            result = {
                "tools": [
                    {
                        "name": "lookup",
                        "description": "Deterministic local fixture.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {},
                            "additionalProperties": False,
                        },
                    }
                ]
            }
        elif method == "tools/call":
            result = {
                "content": [{"type": "text", "text": f"NATIVE_EXTERNAL_OK pid={os.getpid()}"}]
            }
        elif method == "ping":
            result = {}
        else:
            raise RuntimeError("unsupported fixture MCP method")
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)


if "--stdio-docs" in sys.argv:
    stdio_docs()
    raise SystemExit()

parser = argparse.ArgumentParser()
parser.add_argument("--workspace", type=Path, required=True)
parser.add_argument("--receipts", type=Path, required=True)
parser.add_argument("--identity-binary", type=Path, required=True)
parser.add_argument("--ready", type=Path, required=True)
parser.add_argument("--case", default="parallel")
args = parser.parse_args()
steps = {}
lock = threading.Lock()
children_started = threading.Barrier(2, timeout=5)


def receipt(row):
    with lock:
        append_line(args.receipts, json.dumps(row) + "\n")


def fail(reason):
    receipt({"kind": "fixture_error", "reason": reason})
    raise RuntimeError(reason)


def wait_for_parent():
    deadline = time.monotonic() + 5
    while not (args.workspace / "native-parent-delivered").exists():
        if time.monotonic() >= deadline:
            fail("parent response barrier timed out")
        time.sleep(0.01)


def tool(identity, name, parameters):
    return {"type": "tool_use", "id": identity, "name": name, "input": parameters}


def native_agent(marker):
    return {
        "description": "Private native fixture",
        "prompt": marker,
        "subagent_type": "general-purpose",
        "model": "haiku",
        "run_in_background": False,
    }


def role(body):
    system = json.dumps(body.get("system", ""))
    for marker in (
        "NATIVE_CHILD_A",
        "NATIVE_CHILD_B",
        "NATIVE_SKILL_CHILD",
        "NATIVE_INTERRUPT_CHILD",
    ):
        if marker in system:
            return marker
    for message in reversed(body.get("messages", [])):
        if message.get("role") != "user":
            continue
        content = message.get("content", [])
        text = (
            content
            if isinstance(content, str)
            else "\n".join(part.get("text", "") for part in content if part.get("type") == "text")
        )
        for marker in (
            "NATIVE_CHILD_A",
            "NATIVE_CHILD_B",
            "NATIVE_SKILL_CHILD",
            "NATIVE_INTERRUPT_CHILD",
            "NATIVE_ROOT_PARALLEL",
            "NATIVE_ROOT_SKILL",
            "NATIVE_ROOT_INTERRUPT",
            "NATIVE_ROOT_AFTER",
            "NATIVE_ROOT_BRANCH",
            "NATIVE_ROOT_RESUME",
        ):
            if marker in text:
                return marker
    fail("unrecognized native fixture request")
    return None


def response(body):
    if not body.get("tools"):
        # Native title/auxiliary calls are not the scripted main tool loop.
        return [{"type": "text", "text": "Native acceptance fixture"}]
    who = role(body)
    phase = observe_request(body, who)
    handlers = {
        "NATIVE_ROOT_PARALLEL": root_parallel_response,
        "NATIVE_ROOT_SKILL": root_skill_response,
        "NATIVE_SKILL_CHILD": skill_child_response,
        "NATIVE_ROOT_INTERRUPT": root_interrupt_response,
        "NATIVE_INTERRUPT_CHILD": interrupt_child_response,
        "NATIVE_ROOT_AFTER": root_after_response,
        "NATIVE_ROOT_BRANCH": root_branch_response,
        "NATIVE_ROOT_RESUME": root_resume_response,
        "NATIVE_CHILD_A": child_a_response,
        "NATIVE_CHILD_B": child_b_response,
    }
    return handlers[who](phase)


def external_process_id(results):
    identity = None
    for part in results:
        if part.get("tool_use_id") == "native-child-external" and not part.get("is_error", False):
            match = re.search(
                r"NATIVE_EXTERNAL_OK pid=([1-9][0-9]{0,9})\b", json.dumps(part.get("content"))
            )
            if match:
                identity = int(match[1])
    return identity


def inherited_root(body, result_ids):
    tools = {"native-owner-a", "native-owner-b", "native-parent-sdk"}
    if args.case == "purge":
        tools.add("native-large-output")
    messages = json.dumps(body.get("messages", []))
    markers = ("NATIVE_ROOT_PARALLEL", "NATIVE_ROOT_DONE", *sorted(tools))
    return tools <= result_ids and all(marker in messages for marker in markers)


def observe_request(body, who):
    results = [
        part
        for message in body.get("messages", [])
        if isinstance(message.get("content"), list)
        for part in message["content"]
        if part.get("type") == "tool_result"
    ]
    external_ok = any(
        part.get("tool_use_id") == "native-child-external"
        and not part.get("is_error", False)
        and "NATIVE_EXTERNAL_OK" in json.dumps(part.get("content"))
        for part in results
    )
    external_pid = external_process_id(results)
    parent_sdk_ok = any(
        part.get("tool_use_id") == "native-parent-sdk" and not part.get("is_error", False)
        for part in results
    )
    result_ids = {part.get("tool_use_id") for part in results}
    progress = {
        "NATIVE_ROOT_PARALLEL": ("native-parent-sdk",),
        "NATIVE_ROOT_SKILL": (
            "native-owner-skill", "native-cwd-enter", "native-cwd-use", "native-parent-sdk"
        ),
        "NATIVE_CHILD_A": ("native-child-shell",),
        "NATIVE_CHILD_B": ("native-child-write", "native-child-edit"),
        "NATIVE_SKILL_CHILD": ("native-skill-shell",),
        "NATIVE_ROOT_INTERRUPT": ("native-owner-interrupt",),
        "NATIVE_INTERRUPT_CHILD": ("native-interrupt-shell",),
        "NATIVE_ROOT_AFTER": (),
        "NATIVE_ROOT_BRANCH": (),
        "NATIVE_ROOT_RESUME": (),
    }
    phase = sum(identity in result_ids for identity in progress[who])
    with lock:
        key = (who, phase)
        repeated = key in steps
        steps[key] = True
        over_limit = len(steps) > 32
    if repeated or over_limit:
        fail("repeated or excessive native fixture request")
    receipt(
        {
            "kind": "request",
            "role": who,
            "phase": phase,
            "bash_cwd_guidance": "Bash directory changes do not persist between calls."
            in json.dumps(body.get("system", "")),
            "inherited_root": inherited_root(body, result_ids),
            "model": body.get("model")
            if isinstance(body.get("model"), str) and len(body["model"]) <= 128
            else None,
            "external_ok": external_ok,
            "external_pid": external_pid,
            "parent_sdk_ok": parent_sdk_ok,
            "failed_tools": [
                part.get("tool_use_id")
                for part in results
                if part.get("tool_use_id") in ("native-child-nested", "native-child-skill")
                and part.get("is_error") is True
            ],
            "tools": [
                entry.get("name")
                for entry in body.get("tools", [])[:256]
                if isinstance(entry.get("name"), str) and len(entry["name"]) <= 128
            ],
        }
    )
    if who in ("NATIVE_CHILD_A", "NATIVE_CHILD_B") and phase == 0:
        children_started.wait()
        receipt({"kind": "children_concurrent", "role": who})
    if (who == "NATIVE_CHILD_A" and phase == 1) or (who == "NATIVE_CHILD_B" and phase == 2):
        wait_for_parent()
    return phase


def root_parallel_response(phase):
    if phase == 0:
        blocks = [
            tool("native-owner-a", "Agent", native_agent("NATIVE_CHILD_A")),
            tool("native-owner-b", "Agent", native_agent("NATIVE_CHILD_B")),
            tool("native-parent-sdk", "mcp__hive__hive_agents", {}),
        ]
        if args.case == "purge":
            blocks.append(tool("native-large-output", "Bash", {"command": "printf '%080000d' 0"}))
        return blocks
    return [{"type": "text", "text": "NATIVE_ROOT_DONE"}]


def root_skill_response(phase):
    if phase == 0:
        return [tool("native-owner-skill", "Skill", {"skill": "native-fixture:native-canary"})]
    if phase == 1:
        command = "mkdir -p 'native cwd' && cd 'native cwd' && printf nested > same-call"
        return [tool("native-cwd-enter", "Bash", {"command": command})]
    if phase == 2:
        return [tool("native-cwd-use", "Bash", {"command": "printf root > next-call"})]
    if phase == 3:
        return [tool("native-parent-sdk", "mcp__hive__hive_agents", {})]
    return [{"type": "text", "text": "NATIVE_SKILL_ROOT_DONE"}]


def skill_child_response(phase):
    if phase == 0:
        return [
            tool("native-skill-sdk", "mcp__hive__hive_agents", {}),
            tool(
                "native-skill-shell", "Bash", {"command": "printf native-skill > native-skill.txt"}
            ),
        ]
    return [{"type": "text", "text": "NATIVE_PRIVATE_DONE"}]


def root_interrupt_response(phase):
    if phase == 0:
        return [tool("native-owner-interrupt", "Agent", native_agent("NATIVE_INTERRUPT_CHILD"))]
    return fail("interrupted native work unexpectedly completed")


def interrupt_child_response(phase):
    if phase == 0:
        return [
            tool(
                "native-interrupt-shell",
                "Bash",
                {
                    "command": "HIVE_NATIVE_IDENTITY_CHILD=1 exec "
                    + shlex.join(
                        [
                            str(args.identity_binary),
                            "--ignored",
                            "--exact",
                            "api_keys::native_claude::identity::native_shell_identity",
                        ]
                    )
                },
            )
        ]
    return fail("interrupted native work unexpectedly completed")


def root_after_response(phase):
    return [{"type": "text", "text": "NATIVE_AFTER_DONE"}]


def root_branch_response(phase):
    return [{"type": "text", "text": "NATIVE_BRANCH_DONE"}]


def root_resume_response(phase):
    return [{"type": "text", "text": "NATIVE_RESUME_DONE"}]


def child_a_response(phase):
    if phase == 0:
        return [
            tool("native-child-sdk", "mcp__hive__hive_agents", {}),
            tool("native-child-shell", "Bash", {"command": "printf native-a > native-a.txt"}),
            tool("native-child-nested", "Agent", native_agent("NATIVE_FORBIDDEN_CHILD")),
            tool("native-child-skill", "Skill", {"skill": "native-fixture:native-canary"}),
            tool("native-child-external", "mcp__docs__lookup", {}),
        ]
    return [{"type": "text", "text": "NATIVE_PRIVATE_DONE"}]


def child_b_response(phase):
    if phase == 0:
        return [
            tool(
                "native-child-write",
                "Write",
                {"file_path": str(args.workspace / "native-b.txt"), "content": "native-b"},
            )
        ]
    if phase == 1:
        return [
            tool(
                "native-child-edit",
                "Edit",
                {
                    "file_path": str(args.workspace / "native-b.txt"),
                    "old_string": "native-b",
                    "new_string": "native-b-edited",
                },
            )
        ]
    return [{"type": "text", "text": "NATIVE_PRIVATE_DONE"}]


class Handler(BaseHTTPRequestHandler):
    def log_message(self, _format, *_values):
        pass  # Never print URLs, headers, prompts or payloads.

    def do_POST(self):
        self.connection.settimeout(5)
        length = int(self.headers.get("Content-Length", "0"))
        if not 0 < length <= 8 * 1024 * 1024:
            self.send_error(400)
            return
        body = json.loads(self.rfile.read(length))
        if self.path.split("?", 1)[0].endswith("/messages/count_tokens"):
            payload = json.dumps({"input_tokens": 1}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        blocks = response(body)
        message = {
            "id": "msg_native_" + uuid.uuid4().hex,
            "type": "message",
            "role": "assistant",
            "model": body["model"],
            "content": [],
            "stop_reason": None,
            "stop_sequence": None,
            "usage": {
                "input_tokens": 1,
                "output_tokens": 0,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
            },
        }
        stop = "tool_use" if blocks[0]["type"] == "tool_use" else "end_turn"
        if not body.get("stream"):
            message.update(content=blocks, stop_reason=stop)
            message["usage"]["output_tokens"] = 1
            payload = json.dumps(message).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        self.event("message_start", {"message": message})
        for index, block in enumerate(blocks):
            start = (
                {**block, "input": {}}
                if block["type"] == "tool_use"
                else {"type": "text", "text": ""}
            )
            self.event("content_block_start", {"index": index, "content_block": start})
            delta = (
                {"type": "input_json_delta", "partial_json": json.dumps(block["input"])}
                if block["type"] == "tool_use"
                else {"type": "text_delta", "text": block["text"]}
            )
            self.event("content_block_delta", {"index": index, "delta": delta})
            self.event("content_block_stop", {"index": index})
        self.event(
            "message_delta",
            {"delta": {"stop_reason": stop, "stop_sequence": None}, "usage": {"output_tokens": 1}},
        )
        self.event("message_stop", {})
        self.close_connection = True

    def event(self, name, fields):
        data = json.dumps({"type": name, **fields})
        self.wfile.write(f"event: {name}\ndata: {data}\n\n".encode())
        self.wfile.flush()


class Server(ThreadingHTTPServer):
    def handle_error(self, _request, _address):
        # A native retry cannot turn a failed scripted step into a passing run.
        receipt({"kind": "fixture_error", "reason": "HTTP handler failed"})


server = Server(("127.0.0.1", 0), Handler)
publish_text(args.ready, json.dumps({"url": f"http://127.0.0.1:{server.server_port}"}))
server.serve_forever()
