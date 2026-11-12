"""Deterministic native Claude stream protocol; never calls a model service."""

import json
import os
import sys
from pathlib import Path

from mock_common import load_scenario
from mock_claude_runtime import ClaudeRuntime

home = Path(os.environ["CLAUDE_CONFIG_DIR"])
args = sys.argv[1:]
if "--version" in args:
    print(
        load_scenario(home / "runtime-scenario.json").get("provider_version", "2.1.287")
        + " (Claude Code)"
    )
    raise SystemExit()
if "auth" in args:
    print(json.dumps({"loggedIn": True, "authMethod": "claude.ai", "apiProvider": "firstParty"}))
    raise SystemExit()

runtime = ClaudeRuntime(home, args)
while True:
    line = runtime.queued_lines.pop(0) if runtime.queued_lines else sys.stdin.readline()
    if not line:
        break
    frame = json.loads(line)
    kind = frame["type"]
    if kind == "control_request":
        runtime.handle_control_request(frame)
    elif kind == "user":
        runtime.handle_user(frame)
    elif kind == "control_response":
        runtime.handle_control_response(frame)
    else:
        raise AssertionError(frame)
