"""Exact native mock dependencies for copied, checkout-independent providers."""

from .files import atomic_copy

CODEX_MODULES = (
    "mock_codex_account",
    "mock_codex_bodies",
    "mock_codex_conversation",
    "mock_codex_runtime",
    "mock_codex_replies",
    "mock_codex_threads",
    "mock_codex_turns",
)
CLAUDE_MODULES = (
    "mock_claude_actions",
    "mock_claude_controls",
    "mock_claude_input",
    "mock_claude_replies",
    "mock_claude_runtime",
    "mock_claude_shell",
    "mock_claude_startup",
    "mock_claude_tasks",
)
NATIVE_MODULES = CODEX_MODULES + CLAUDE_MODULES


def copy_codex_modules(root, destination):
    """Copy each required source beside the standalone Codex entry executable."""
    for module in CODEX_MODULES:
        name = module + ".py"
        atomic_copy(root / "tests/fixtures" / name, destination / name, mode=0o600)
