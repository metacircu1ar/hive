"""Shared scenario defaults and product limits checked against Rust."""

# src/runtime/provider/rpc/terminal_drain.rs; values are milliseconds.
TERMINAL_DRAIN_QUIET_MS = 3_000
TERMINAL_DRAIN_DEADLINE_MS = 20_000

# Mechanical checks use Terra 6.1 at low effort; reasoning roles stay medium.
LIVE_CODEX_MODEL = "gpt-6.1-terra"
LIVE_CLAUDE_MODEL = "claude-sonnet-5-5"
LIVE_CODEX_EFFORT = "low"
LIVE_REASONING_MODEL = "gpt-6.1-terra"
LIVE_REASONING_EFFORT = "medium"
# Exercise a persistent model change on the same conversation.
LIVE_CODEX_CHANGED_EFFORT = "medium"
