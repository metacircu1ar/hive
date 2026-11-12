"""Own installed setup, agent checks and teardown within one temporary home."""

import json
import subprocess
from .process import run as run_process

from .acceptance import ready
from .acceptance import Cli, event_pages, json_command, mock_login, require, running, wait_for
from .common import write_json
from .common import digest
from .files import raw_log


class InstalledWizard:
    def __init__(self, owner, home, url):
        self.owner = owner
        self.home = home
        self.url = url

    def run(self):
        self.prepare_paths()
        self.configure()
        self.check_configuration()
        self.exercise()
        self.capture_upgrade()
        self.remove_selected()
        self.purge_root()
        return {
            "setup": "configured",
            "identical": "unchanged",
            "conflict": "preserved",
            "doctor": self.doctor,
            "default_agent_comb_tools": self.replies,
            "http_vector_search": "indexed",
            "upgrade": "preserved",
            "plain_uninstall": "retained",
            "purge": "retained knowledge/auth; repeat no-op",
            "explicit_root_purge": "removed",
            "installed_helper_removal": "stable uninstall; retained-image purge; stable root purge",
        }

    def prepare_paths(self):
        self.workspace = self.home / "repo"
        self.workspace.mkdir()
        self.config, self.state = self.home / "config.yaml", self.home / "state"
        self.root, self.bridge, self.embedding = (
            self.home / "knowledge",
            self.home / "bridge.json",
            self.home / "embedding.json",
        )
        write_json(
            self.embedding,
            {
                "backend": "http",
                "model": "fixture",
                "revision": "pinned",
                "url": self.url,
                "dimensions": 2,
            },
        )

    def configure(self):
        self.global_args = [
            "--config",
            str(self.config),
            "--state-dir",
            str(self.state),
            "--codex-bin",
            str(self.owner.provider),
        ]
        self.answers = [
            "--yes",
            "--provider",
            "codex",
            "--codex-profile",
            "mock",
            "--login",
            "skip",
            "--codex-model",
            "gpt-5.6-luna",
            "--codex-effort",
            "medium",
            "--no-services",
            "--comb-root",
            str(self.root),
            "--bridge-file",
            str(self.bridge),
            "--embedding-config",
            str(self.embedding),
        ]
        self.label, self.variant = "wizard", "--comb"
        self.prefix = self.owner.install(
            self.label, self.variant, "--yes", "--", *self.global_args, *self.answers, setup=True
        )
        self.owner.verify(self.prefix, "hive")
        self.owner.verify(self.prefix, "comb")
        self.command = [str(self.prefix / "bin/hivectl"), *self.global_args]
        self.remote = [self.command[0], "--state-dir", str(self.state)]
        self.control = self.remote + ["--config", str(self.config)]
        self.routing = Cli(
            self.remote,
            prefixes={"daemon": self.control, "setup": self.command, "doctor": self.command},
        )
        self.call = lambda *args: json_command(self.routing.argv(*args), env=self.owner.env)
        mock_login(self.command, env=self.owner.env)

    def check_configuration(self):
        self.pinned = [self.config, self.bridge, self.root / ".comb/serve.json"]
        self.before = [(p.read_bytes(), p.stat().st_mtime_ns) for p in self.pinned]
        again = self.call("setup", *self.answers)
        require(again["configured"] and not again["changed"], again)
        require(
            self.before == [(p.read_bytes(), p.stat().st_mtime_ns) for p in self.pinned],
            "Identical setup rewrote owned settings",
        )
        conflicting = self.answers.copy()
        conflicting[conflicting.index("--codex-model") + 1] = "conflicting-model"
        conflict = run_process(
            self.command + ["setup", *conflicting],
            env=self.owner.env,
            capture_output=True,
            text=True,
            timeout=30,
            stop_grace=12,
            check=False,
        )
        require(
            conflict.returncode != 0 and self.config.read_bytes() == self.before[0][0],
            "Setup conflict changed configuration",
        )
        require(not list(self.home.glob("config.yaml.backup*")), "Conflict created a backup")

    def exercise_agent(self, daemon):
        wait_for(lambda: ready(self.call, "daemon", "status"), "wizard daemon startup", daemon)
        self.doctor = self.call("doctor")
        require(all(c["status"] != "error" for c in self.doctor["checks"]), self.doctor)
        actions = [
            {
                "name": "comb_put",
                "arguments": {
                    "slug": "concepts/wizard",
                    "markdown": "---\ntitle: Wizard\n---\nPackaged setup works.",
                },
            },
            {"name": "comb_search", "arguments": {"query": "Packaged", "mode": "text"}},
        ]
        self.agent = self.call(
            "spawn", "--cwd", str(self.workspace), "--body", "comb:" + json.dumps(actions)
        )["agent_id"]

        def finished():
            events = event_pages(self.call, self.agent)
            outputs = [e["data"]["text"] for e in events if isinstance(e["data"].get("text"), str)]
            return (
                outputs[-1]
                if outputs
                and any(
                    e["type"] == "turn_finished" and e["data"].get("status") == "completed"
                    for e in events
                )
                else None
            )

        output = wait_for(finished, "wizard default agent Comb turn", daemon)
        require(output.startswith("comb-results:"), output)
        self.replies = json.loads(output.removeprefix("comb-results:"))
        require(
            len(self.replies) == 2 and all("error" not in r for r in self.replies), self.replies
        )
        require(self.replies[1]["results"][0]["slug"] == "concepts/wizard", self.replies)
        self.call("terminate", self.agent)

    def check_index(self, server):
        def indexed():
            found = json_command(
                self.comb + ["search", "Packaged", "--mode", "vector"], env=self.owner.env
            )
            require(found["mode"] == "vector", found)
            return (
                found
                if found["results"] and found["results"][0]["slug"] == "concepts/wizard"
                else None
            )

        wait_for(indexed, "packaged HTTP embedding background indexing", server)

    def exercise(self):
        self.comb = [str(self.prefix / "bin/comb"), "--root", str(self.root)]
        with running(
            self.comb + ["serve"], self.owner.output / f"{self.label}-comb.log", self.owner.env
        ) as server:
            wait_for(
                lambda: (
                    self.root.joinpath(".comb/service.sock").exists()
                    and ready(json_command, self.comb + ["stats"], env=self.owner.env)
                ),
                "wizard Comb startup",
                server,
            )
            with running(
                self.command + ["daemon", "run"],
                self.owner.output / f"{self.label}-daemon.log",
                self.owner.env,
            ) as daemon:
                self.exercise_agent(daemon)
                self.check_index(server)
                self.call("daemon", "stop")
                require(daemon.wait(timeout=20) == 0, "Wizard daemon stop failed")

    def capture_upgrade(self):
        self.retained = {
            p: p.read_bytes()
            for p in [
                *self.pinned,
                self.root / "concepts/wizard.md",
                *self.state.joinpath("profiles").rglob("*"),
            ]
            if p.is_file()
        }
        require(
            any(p.is_relative_to(self.state / "profiles") for p in self.retained),
            "No profile evidence captured",
        )
        self.owner.install(self.label, self.variant, "--yes")
        require(
            all(p.read_bytes() == data for p, data in self.retained.items()),
            "Upgrade changed retained data",
        )
        self.installer = self.prefix / "bin/hive-install"
        self.retained_installer = self.installer.resolve(strict=True)
        self.check_installer()
        removal_args = [
            "--prefix",
            str(self.prefix),
            "--uninstall",
            "--comb",
            "--yes",
        ]
        self.remove = [str(self.installer), *removal_args]
        retry_remove = [
            "sh",
            str(self.owner.bootstrap),
            "--from",
            str(self.owner.assets),
            *removal_args,
        ]
        purge_args = [
            "--purge",
            "--config",
            str(self.config),
            "--state-dir",
            str(self.state),
            "--comb-root",
            str(self.root),
            "--bridge-file",
            str(self.bridge),
        ]
        self.purge = [str(self.retained_installer), *removal_args, *purge_args]
        self.retry_purge = [*retry_remove, *purge_args]
        self.log = self.owner.output / f"{self.label}-removal.log"

    def check_installer(self):
        expected = self.owner.identities["hive-install"]
        require(
            self.retained_installer.stat().st_size == expected["bytes"]
            and digest(self.retained_installer) == expected["sha256"],
            "Installed maintenance helper differs from the admitted release",
        )

    def check_removed_entrypoints(self):
        parents = {child.name: child for child in self.prefix.iterdir()}
        for label, names in (
            ("bin", {"hivectl", "hive-daemon", "comb", "comb-llama-server", "hive-install"}),
            ("share", {"hive", "comb", "comb-llama"}),
        ):
            parent = parents.get(label)
            if parent is not None:
                require(
                    names.isdisjoint(child.name for child in parent.iterdir()),
                    "Selected entrypoints remain after plain removal",
                )

    def remove_selected(self):
        with raw_log(self.log) as stream:
            run_process(
                self.remove,
                env=self.owner.env,
                check=True,
                stdout=stream,
                stderr=subprocess.STDOUT,
                timeout=120,
                stop_grace=12,
            )
            require(
                all(p.read_bytes() == data for p, data in self.retained.items()),
                "Plain removal changed retained data",
            )
            self.check_removed_entrypoints()
            self.check_installer()
            run_process(
                self.purge,
                env=self.owner.env,
                check=True,
                stdout=stream,
                stderr=subprocess.STDOUT,
                timeout=120,
                stop_grace=12,
            )
            self.check_purge()
            run_process(
                self.retry_purge,
                env=self.owner.env,
                check=True,
                stdout=stream,
                stderr=subprocess.STDOUT,
                timeout=120,
                stop_grace=12,
            )
            self.check_purge()

    def check_purge(self):
        for path in (self.config, self.bridge, self.prefix / "lib/hive"):
            try:
                path.lstat()
            except FileNotFoundError:
                continue
            raise RuntimeError("Full purge left selected payload")
        retained_after = {
            p: data
            for p, data in self.retained.items()
            if p == self.root / "concepts/wizard.md" or p.is_relative_to(self.state / "profiles")
        }
        require(
            all(p.read_bytes() == data for p, data in retained_after.items()),
            "Purge changed retained knowledge/auth",
        )

    def purge_root(self):
        self.owner.install(self.label, "--standalone-comb", "--yes")
        self.retained_installer = self.installer.resolve(strict=True)
        self.check_installer()
        explicit = [
            str(self.prefix / "bin/hive-install"),
            "--prefix",
            str(self.prefix),
            "--uninstall",
            "--standalone-comb",
            "--purge",
            "--yes",
            "--comb-root",
            str(self.root),
            "--purge-comb-root",
            str(self.root),
        ]
        with raw_log(self.log) as stream:
            run_process(
                explicit,
                env=self.owner.env,
                check=True,
                stdout=stream,
                stderr=subprocess.STDOUT,
                timeout=120,
                stop_grace=12,
            )
        require(not self.root.exists(), "Explicit knowledge-root purge did not remove it")
