"""Per-conversation state, distinct from the native process runtime."""

import subprocess
from dataclasses import dataclass, field


@dataclass
class Conversation:
    thread_id: str | None = None
    turn_id: str | None = None
    active_turn: bool = False
    meta: dict = field(default_factory=dict)
    pending: str | None = None
    pending_args: dict | None = None
    steering_request: dict | None = None
    actions: list = field(default_factory=list)
    command_child: subprocess.Popen | None = None
    list_calls: int = 0
