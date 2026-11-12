"""Own one bounded MCP subprocess transcript and its cleanup."""

import json
import subprocess

from .acceptance import require
from .owned_process import spawn
from .cancellation import deadline


class McpClient:
    def __init__(self, command, env):
        self.command = command
        self.env = env

    def call(self, tool, arguments, *, offer="2025-11-25", selected="2025-11-25"):
        requests = [
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": offer,
                    "capabilities": {},
                    "clientInfo": {"name": "compat", "version": "synthetic-0"},
                },
            },
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": tool, "arguments": arguments},
            },
        ]
        with spawn(self.command, stdin=subprocess.PIPE, env=self.env, limit=1024 * 1024) as process:
            process.send("".join(json.dumps(r) + "\n" for r in requests).encode())
            with deadline(30):
                replies = [json.loads(process.line(timeout=30)) for _ in range(2)]
            process.send(b"", close=True)
            require(process.wait(timeout=5) == 0, "MCP process failed")
            require(not process.output.strip(), "MCP transcript had extra replies")
        require(
            len(replies) == 2 and replies[0]["id"] == 1 and replies[1]["id"] == 2,
            "MCP transcript lost a reply",
        )
        require(replies[0]["result"]["protocolVersion"] == selected, "MCP negotiation mismatch")
        result = replies[1]["result"]
        require(not result.get("isError", False), "MCP operation failed")
        return json.loads(result["content"][0]["text"])
