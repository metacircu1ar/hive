import json
import os
import pathlib
import signal
import sys
import time

from mock_codex_conversation import Conversation
from mock_codex_runtime import CodexRuntime

from mock_common import (
    load_scenario,
    wait_for_gate,
    publish_text,
    append_line,
)

from mock_common import schema_probe

if schema_probe(sys.argv):
    raise SystemExit()

if "sandbox" in sys.argv:
    # Mock diagnostic routing only; native acceptance is a separate opt-in test.
    assert sys.argv[-4:] == ["sandbox", "--", "/bin/echo", "hive-native-sandbox-ready"]
    print("hive-native-sandbox-ready")
    raise SystemExit()

home = pathlib.Path(os.environ["CODEX_HOME"])
scenario = load_scenario(home / "runtime-scenario.json")
if "--version" in sys.argv:
    version = scenario.get("provider_version", "0.163.0")
    print("codex-cli " + version)
    raise SystemExit()
append_line(
    home / "process-options.jsonl",
    json.dumps(
        {
            "pid": os.getpid(),
            "compression": [
                arg for arg in sys.argv if arg.startswith("features.enable_request_compression=")
            ],
        }
    )
    + "\n",
)
if scenario.get("require_sigterm"):
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
if scenario.get("close_fds"):
    os.closerange(3, 1024)
if scenario.get("pid_file"):
    publish_text(pathlib.Path(scenario["pid_file"]), str(os.getpid()))

runtime = CodexRuntime(home, scenario)
conversation = Conversation()
for line in sys.stdin:
    request = json.loads(line)
    conversation = runtime.select_conversation(request, conversation)
    method = request.get("method")
    if method and scenario.get("trace"):
        append_line(scenario["trace"], method + "\n")
    if method is None and "result" in request:
        handler = runtime.handle_tool_result
    elif method and "id" in request:
        handler = runtime.handlers.get(method, runtime.handle_unsupported)
    else:
        handler = runtime.handlers.get(method)
    if handler is not None and handler(request, conversation) is False:
        break


if scenario.get("linger_eof") and conversation.thread_id is not None:
    if scenario.get("eof_stop_file"):
        wait_for_gate(scenario["eof_stop_file"], timeout=15)
    else:
        time.sleep(3)
if scenario.get("malformed_eof") and conversation.thread_id is not None:
    print("malformed protocol", flush=True)
if scenario.get("unexpected_eof") and conversation.thread_id is not None:
    runtime.emit(
        {
            "method": "turn/started",
            "params": {
                "threadId": conversation.thread_id,
                "turn": {"id": "unexpected", "status": "inProgress"},
            },
        }
    )
if scenario.get("require_sigterm"):
    time.sleep(30)
