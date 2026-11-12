"""Transparent native CLI transport for opt-in, auth-free local-model fixtures.

Only bounded protocol identities and decisions are recorded. Never retain prompts,
messages, credentials, account replies or native stderr in the observation file.
"""

import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import time
import uuid

from mock_common import append_line

settings = json.loads(Path(__file__).with_name("native-fixture.json").read_text())
binary = settings["binary"]
os.environ["ANTHROPIC_BASE_URL"] = settings["url"]
for name in (
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_CUSTOM_HEADERS",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
):
    os.environ.pop(name, None)
os.environ["NO_PROXY"] = "127.0.0.1,localhost,::1"
for name in (
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
    "DISABLE_TELEMETRY",
    "DISABLE_ERROR_REPORTING",
    "DISABLE_AUTOUPDATER",
):
    os.environ[name] = "1"

if "--version" in sys.argv:
    os.execv(binary, [binary, *sys.argv[1:]])

lock = threading.Lock()
trace = Path(settings["trace"])
requests = {}
tasks = {}
open_owners = set()
held = {}
held_since = None
parent_request = None
failed = threading.Event()
stopping = threading.Event()
writer_lock = threading.Lock()
parallel = settings.get("case") in ("parallel", "branch", "purge")
launch = uuid.uuid4().hex


def bounded(value):
    return value if isinstance(value, str) and len(value.encode()) <= 1024 else None


def record(row):
    with lock:
        append_line(trace, json.dumps({"launch": launch, **row}) + "\n")


def observe(frame, native):
    kind = frame.get("type")
    if native and kind == "system" and frame.get("subtype") == "init":
        for server in frame.get("mcp_servers", []):
            if server.get("name") in ("docs", "hive"):
                status = server.get("status")
                record(
                    {
                        "kind": "mcp_server_state",
                        "server": server["name"],
                        "status": status
                        if status in ("connected", "failed", "pending", "disabled")
                        else "unknown",
                    }
                )
    if (
        not native
        and kind == "control_request"
        and frame.get("request", {}).get("subtype") == "interrupt"
    ):
        record({"kind": "interrupt_requested", "request_id": bounded(frame.get("request_id"))})
    if native and kind == "result":
        observe_result(frame)
    if native and kind == "system" and frame.get("subtype", "").startswith("task_"):
        observe_task(frame, kind)
    elif native and kind in ("assistant", "user", "stream_event"):
        observe_message(frame, kind)
    elif native and kind == "control_request":
        observe_control(frame)
    elif not native and kind == "control_response":
        return observe_decision(frame)
    return None


def observe_result(frame):
    record(
        {
            "kind": "native_result",
            "session": bounded(frame.get("session_id")),
            "subtype": bounded(frame.get("subtype")),
            "is_error": frame.get("is_error") is True,
        }
    )


def observe_task(frame, kind):
    record(
        {
            "kind": kind,
            **{
                key: bounded(frame.get(key))
                for key in ("subtype", "task_id", "tool_use_id", "task_type")
            },
            "spawn_depth": frame.get("spawn_depth")
            if type(frame.get("spawn_depth")) is int
            else None,
            "is_backgrounded": frame.get("is_backgrounded")
            if isinstance(frame.get("is_backgrounded"), bool)
            else None,
        }
    )
    if frame.get("subtype") == "task_started":
        with lock:
            tasks[bounded(frame.get("tool_use_id"))] = bounded(frame.get("task_id"))
            open_owners.add(bounded(frame.get("tool_use_id")))


def observe_message(frame, kind):
    record(
        {
            "kind": kind,
            "has_parent": "parent_tool_use_id" in frame,
            "parent": bounded(frame.get("parent_tool_use_id")),
            "session": bounded(frame.get("session_id")),
        }
    )
    if kind == "user" and frame.get("parent_tool_use_id") is None:
        content = frame.get("message", {}).get("content", [])
        if isinstance(content, list):
            for block in content:
                if block.get("type") == "tool_result":
                    close_owner(bounded(block.get("tool_use_id")))


def observe_control(frame):
    global parent_request
    request = frame.get("request", {})
    hook = request.get("input", {}) if request.get("subtype") == "hook_callback" else request
    row = {
        "kind": "control",
        "subtype": bounded(request.get("subtype")),
        "hook": bounded(hook.get("hook_event_name")),
        "tool": bounded(hook.get("tool_name")),
        "tool_id": bounded(hook.get("tool_use_id")),
        "agent_id": bounded(hook.get("agent_id")),
        "empty_arguments": hook.get("tool_input") == {},
    }
    with lock:
        if len(requests) >= 512:
            raise RuntimeError("native fixture observation bound")
        identity = bounded(frame.get("request_id"))
        if identity is None:
            raise RuntimeError("invalid native fixture identity")
        requests[identity] = row
    record(row)
    if row["hook"] in ("PostToolUse", "PostToolUseFailure") and row["agent_id"] is None:
        close_owner(row["tool_id"])
    if request.get("subtype") == "mcp_message":
        message = request.get("message", {})
        if message.get("method") == "tools/call":
            name = bounded(message.get("params", {}).get("name"))
            record(
                {
                    "kind": "sdk_dispatch",
                    "tool": name,
                    "request_id": identity,
                    "empty_arguments": message.get("params", {}).get("arguments") == {},
                }
            )
            if parallel and name == "hive_agents" and parent_request is None:
                parent_request = identity


def observe_decision(frame):
    response = frame.get("response", {})
    with lock:
        request = requests.pop(response.get("request_id"), None)
    if request:
        output = response.get("response", {})
        specific = output.get("hookSpecificOutput", {})
        updated = specific.get("updatedInput", {})
        decision = specific.get("permissionDecision", output.get("behavior"))
        record(
            {
                **request,
                "kind": "decision",
                "decision": decision if decision in ("allow", "deny", "ask") else None,
                "foreground": updated.get("run_in_background") is False,
            }
        )
        return {**request, "decision": decision}
    return None


def close_owner(identity):
    with lock:
        present = identity in open_owners
        open_owners.discard(identity)
    if present:
        record({"kind": "owner_closed", "tool_id": identity})


def forward_input():
    global held_since
    for line in sys.stdin.buffer:
        frame = json.loads(line)
        decision = observe(frame, False)
        response_id = frame.get("response", {}).get("request_id")
        side = None
        if parallel and response_id is not None and response_id == parent_request:
            side = "parent"
        elif (
            parallel
            and decision
            and decision["tool_id"] == "native-child-sdk"
            and decision["hook"] == "PreToolUse"
        ):
            if decision["decision"] != "deny":
                raise RuntimeError("child SDK request was admitted")
            side = "child"
        if side:
            with lock:
                if (
                    side in held
                    or sum(len(data) for data in held.values()) + len(line) > 2 * 1024 * 1024
                ):
                    raise RuntimeError("native response gate bound")
                held[side] = line
                held_since = held_since or time.monotonic()
            record({"kind": "response_held", "side": side, "request_id": bounded(response_id)})
        else:
            write_native(line)
    child.stdin.close()


def forward_output():
    for line in child.stdout:
        observe(json.loads(line), True)
        sys.stdout.buffer.write(line)
        sys.stdout.buffer.flush()


def terminate(_number, _frame):
    stopping.set()


def write_native(line):
    with writer_lock:
        child.stdin.write(line)
        child.stdin.flush()


def supervised(name, action):
    complete = False
    try:
        action()
        complete = True
    finally:
        if not complete:
            failed.set()
            record({"kind": "transport_error", "worker": name})


def release_responses():
    global held_since
    with lock:
        if held_since and time.monotonic() - held_since > 5:
            raise RuntimeError("native overlap gate timed out")
        if not (
            {"parent", "child"} <= held.keys()
            and {"native-owner-a", "native-owner-b"} <= open_owners
        ):
            return
        child_reply = held.pop("child")
        parent_reply = held.pop("parent")
        held_since = None
    record({"kind": "overlap_established"})
    write_native(child_reply)
    record({"kind": "child_denial_delivered"})
    write_native(parent_reply)
    record({"kind": "parent_response_delivered"})
    # This releases only the scripted inference response, never a permission.
    from mock_common import publish_text

    publish_text(Path(settings["workspace"]) / "native-parent-delivered", "ready")


def stop_child():
    if child.poll() is None:
        try:
            child.terminate()
        except ProcessLookupError:
            child.wait(timeout=5)
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=5)


signal.signal(signal.SIGTERM, terminate)
record({"kind": "capture_started"})
child = subprocess.Popen(
    [binary, *sys.argv[1:], "--plugin-dir", settings["plugin"]],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=sys.stderr.buffer,
)
threading.Thread(target=supervised, args=("input", forward_input), daemon=True).start()
output = threading.Thread(target=supervised, args=("output", forward_output), daemon=True)
output.start()
complete = False
try:
    while child.poll() is None and not failed.is_set() and not stopping.is_set():
        release_responses()
        time.sleep(0.01)
    complete = not failed.is_set()
finally:
    stop_child()
    output.join(timeout=5)
    ok = (
        complete
        and not failed.is_set()
        and not output.is_alive()
        and not held
        and (child.returncode == 0 or stopping.is_set())
    )
    record({"kind": "capture_complete", "ok": ok, "native_status": child.returncode})
raise SystemExit(child.returncode if ok else 1)
