"""Claude fixture startup on its single protocol runtime."""

import json
import os
from pathlib import Path
from mock_common import (
    append_line,
)


class Startup:
    def initialize(self):
        self.select_session()
        if self.session:
            self.record_launch()
        self.initialize_native_history()
        self.turn = None
        self.mode = None
        self.counter = 0
        self.pending = {}
        self.comb_actions = []
        self.comb_results = []
        self.queued_lines = []
        self.shell_process = None
        self.agent = (
            Path(self.option("--append-system-prompt-file"))
            .read_text()
            .split("agent ", 1)[1]
            .split(",", 1)[0]
            if self.session
            else None
        )

    def select_session(self):
        assert "app-server" not in self.args
        assert self.option("--setting-sources") == ""
        if self.scenario().get("api_key"):
            assert os.environ.get("ANTHROPIC_API_KEY", "").startswith("private-api-")
            assert "ANTHROPIC_AUTH_TOKEN" not in os.environ
            assert "OPENAI_API_KEY" not in os.environ and "CODEX_API_KEY" not in os.environ
            assert os.environ["ANTHROPIC_API_KEY"] not in self.args
        self.native_model = (
            "claude-haiku-4-5"
            if "--model" in self.args and self.option("--model") in ("haiku", "claude-haiku-4-5")
            else "claude-sonnet-5"
        )
        self.fork = "--fork-session" in self.args
        self.session = (
            self.option("--session-id")
            if self.fork
            else (
                self.option("--resume")
                if "--resume" in self.args
                else self.option("--session-id")
                if "--session-id" in self.args
                else None
            )
        )
        self.saved = self.home / ("mock-session-" + str(self.session))
        if ("--resume" in self.args and not self.fork and not self.saved.exists()) or (
            "--session-id" in self.args and self.saved.exists()
        ):
            self.emit(
                {
                    "type": "result",
                    "session_id": "native-startup-id",
                    "subtype": "error_during_execution",
                    "is_error": True,
                    "num_turns": 0,
                }
            )
            raise SystemExit(1)
        if self.fork:
            assert (self.home / ("mock-session-" + self.option("--resume"))).exists()
        self.history_file = self.home / ("mock-history-" + str(self.session))
        self.history = (
            json.loads(self.history_file.read_text()) if self.history_file.exists() else []
        )
        if self.fork:
            self.history = json.loads(
                (self.home / ("mock-history-" + self.option("--resume"))).read_text()
            )

    def record_launch(self):
        assert os.environ["CLAUDE_CODE_DISABLE_BACKGROUND_TASKS"] == "1"
        settings = json.loads(self.option("--settings"))
        assert settings["sandbox"]["enabled"] and settings["sandbox"]["failIfUnavailable"]
        assert not settings["sandbox"]["allowUnsandboxedCommands"]
        assert "--permission-prompt-tool" in self.args and "--replay-user-messages" in self.args
        append_line(
            self.home / "mock-launches.jsonl",
            json.dumps(
                {
                    "session": self.session,
                    "resume": "--resume" in self.args,
                    "pid": os.getpid(),
                    "model": self.native_model,
                    "effort": self.option("--effort") if "--effort" in self.args else "none",
                    "settings": settings,
                    "environment": {
                        key: os.environ.get(key)
                        for key in (
                            "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING",
                            "MAX_THINKING_TOKENS",
                            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
                            "MAX_MCP_OUTPUT_TOKENS",
                        )
                    },
                    "instructions": Path(self.option("--append-system-prompt-file")).read_text(),
                }
            )
            + "\n",
        )

    def initialize_native_history(self):
        import unicodedata

        cwd_units = unicodedata.normalize("NFC", str(Path.cwd().resolve())).encode("utf-16-le")
        units = [
            int.from_bytes(cwd_units[i : i + 2], "little") for i in range(0, len(cwd_units), 2)
        ]
        project = "".join(
            chr(unit) if unit < 128 and chr(unit).isalnum() else "-" for unit in units
        )
        if len(project) > 200:
            value = 0
            for unit in units:
                value = (value * 31 + unit) & 0xFFFFFFFF
            value = abs(value if value < 0x80000000 else value - 0x100000000)
            suffix = ""
            while value:
                suffix = "0123456789abcdefghijklmnopqrstuvwxyz"[value % 36] + suffix
                value //= 36
            project = project[:200] + "-" + (suffix or "0")
        self.native_history_file = self.home / "projects" / project / (str(self.session) + ".jsonl")
        self.native_history = []
        (self.home / "projects").mkdir(mode=0o700, exist_ok=True)
        self.native_history_file.parent.mkdir(mode=0o700, exist_ok=True)
        native_source = self.native_history_file
        if self.fork:
            native_source = native_source.with_name(self.option("--resume") + ".jsonl")
        if native_source.exists():
            self.native_history = [
                json.loads(line) for line in native_source.read_text().splitlines()
            ]
        if self.fork and self.scenario().get("branch_bad_history"):
            self.native_history = []
