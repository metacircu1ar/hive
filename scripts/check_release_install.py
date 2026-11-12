#!/usr/bin/env python3
"""Native installed-release acceptance: deterministic agents and embedding endpoint.

Run after build_release.py/package_release.py assemble all five native parts.
No real provider credentials, external network calls, real models or Cargo test binaries.
"""

import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import subprocess
from release.process import run as run_process
import sys
import time

from release.acceptance import ready
from release.acceptance import (
    Cli,
    event_data,
    event_pages,
    mock_login,
    require,
    wait_for,
    running,
    json_command,
)
from release.installed_graph import check_graph
from release.installed_wizard import InstalledWizard
from release.asset_admission import admit
from release.embedding_fixture import embedding_server
from release.files import atomic_write_text, atomic_copy, create_directory, raw_log
from release import llama
from release.common import (
    PARTS,
    ROOT,
    component,
    dependencies_match,
    digest,
    inspect_binary,
    native_target,
    write_json,
)
from release.progress import entry, phase
from release.cancellation import finish
from release.temporary import directory as private_directory
from release.native_evidence import evidence
from release.mock_provider import copy_codex_modules


class InstalledChecks:
    def __init__(self, assets, output, work):
        self.output, self.work = output, work
        self.assets, helper = admit(assets, work / "assets")
        self.env = {
            k: v
            for k, v in os.environ.items()
            if not k.startswith("DYLD_") and k != "LD_LIBRARY_PATH"
        }
        self.bootstrap = self.assets / "install.sh"
        self.provider = work / "mock-provider"
        atomic_write_text(
            self.provider,
            f"#!{sys.executable}\nimport sys\n"
            'if "--version" in sys.argv:\n    print("codex-cli 0.161.0"); sys.exit(0)\n'
            + (ROOT / "tests/fixtures/runtime_provider.py").read_text(),
            mode=0o700,
        )
        atomic_copy(
            ROOT / "tests/fixtures/codex-schema.json", work / "codex-schema.json", mode=0o600
        )
        atomic_copy(ROOT / "tests/fixtures/mock_common.py", work / "mock_common.py", mode=0o600)
        atomic_copy(ROOT / "scripts/release/files.py", work / "mock_files.py", mode=0o600)
        copy_codex_modules(ROOT, work)
        self.identities = {"hive-install": helper}

    def install(self, label, *flags, setup=False):
        prefix = self.output / f"{label}-prefix"
        with raw_log(self.output / f"{label}-install.log") as log:
            log.write(f"\nInstall {label}: {flags!r}; setup={setup}\n".encode())
            log.flush()
            run_process(
                [
                    "sh",
                    str(self.bootstrap),
                    "--from",
                    str(self.assets),
                    "--prefix",
                    str(prefix),
                    *([] if setup else ["--no-setup"]),
                    *flags,
                ],
                check=True,
                env=self.env,
                stdout=log,
                stderr=subprocess.STDOUT,
                timeout=120,
                stop_grace=12,
            )
        return prefix

    def verify(self, prefix, part):
        package, flags, names, variant = PARTS[part]
        record = json.loads((prefix / "share" / component(part) / "build.json").read_text())
        require(record["part"] == part and record["profile"] == "release", record)
        if part == llama.PART:
            return self.verify_llama(prefix, record)
        require(
            record["command"]
            == [
                "cargo",
                "build",
                "--release",
                "--locked",
                "-p",
                package,
                *flags,
                "--message-format=json-render-diagnostics",
            ],
            record,
        )
        for name in names:
            binary = prefix / "bin" / name
            info = record["binaries"][name]
            require(
                not info["cargo_profile"]["test"] and info["cargo_profile"]["opt_level"] == "3",
                info,
            )
            require(
                digest(binary) == info["sha256"],
                f"Installed {name} differs from plain release build",
            )
            require(
                dependencies_match(
                    info["dependencies"], inspect_binary(binary, native_target()), native_target()
                ),
                "Installed dependency closure differs",
            )
            version = run_process(
                [str(binary), "--version"], text=True, env=self.env, check=True, timeout=30
            ).stdout.strip()
            require(version.endswith(f"({variant})"), version)
        self.identities[part] = {k: record[k] for k in ["source", "target", "binaries"]}

    def verify_llama(self, prefix, record):
        """The installed server is the frozen static build and reports its pinned commit."""
        require(record["command"] == llama.COMMAND, record)
        llama.check_record(record["llama"], native_target())
        binary = prefix / "bin" / llama.BINARY
        info = record["binaries"][llama.BINARY]
        require(
            digest(binary) == info["sha256"],
            "Installed llama-server differs from its frozen release build",
        )
        require(
            dependencies_match(
                info["dependencies"], inspect_binary(binary, native_target()), native_target()
            ),
            "Installed llama-server dependency closure differs",
        )
        version = llama.version(binary, env=self.env)
        llama.check_version(version, record["llama"]["commit"])
        require(version == info["version"], version)
        self.identities[llama.PART] = {k: record[k] for k in ["source", "target", "binaries"]}

    @contextmanager
    def daemon(self, prefix, label, additions=None):
        work = self.work / label
        work.mkdir()
        instructions = work / "role.md"
        atomic_write_text(instructions, "Deterministic installed release fixture.\n")
        config = {
            "auth_profiles": {"mock": {"provider": "codex", "method": "subscription"}},
            "roles": {
                "implementor": {
                    "auth": "mock",
                    "model": "gpt-5.6-luna",
                    "effort": "medium",
                    "instructions": str(instructions),
                    "lifetime": "independent",
                    "sandbox": "read-only",
                }
            },
        }
        config.update(additions or {})
        path = work / "config.json"
        write_json(path, config)
        command = [
            str(prefix / "bin/hivectl"),
            "--config",
            str(path),
            "--state-dir",
            str(work / "state"),
            "--codex-bin",
            str(self.provider),
        ]
        mock_login(command, env=self.env)
        remote = [command[0], "--state-dir", str(work / "state")]
        control = remote + ["--config", str(path)]
        call = Cli(remote, prefixes={"daemon": control, "auth": control}, timeout=10)
        with running(
            command + ["daemon", "run"], self.output / f"{label}-daemon.log", self.env
        ) as process:
            wait_for(lambda: ready(call, "daemon", "status"), f"{label} daemon startup", process)
            try:
                yield call, work, config, path
            finally:
                if not process.exited():
                    call("daemon", "stop")
                    require(process.wait(timeout=20) == 0, f"{label} daemon shutdown failed")

    def turn(self, call, work, body):
        task = work / "task.txt"
        atomic_write_text(task, body)
        agent = call("spawn", "implementor", "--cwd", str(work), "--body-file", str(task))[
            "agent_id"
        ]

        def finished():
            events = event_pages(call, agent)
            data = [event_data(call, e["data"]) for e in events]
            outputs = [e["text"] for e in data if isinstance(e.get("text"), str)]
            if outputs and any(
                e["type"] == "turn_finished" and e["data"].get("status") == "completed"
                for e in events
            ):
                return outputs[-1]
            return None

        result = wait_for(finished, "installed agent turn")
        call("terminate", agent)
        return result

    def core(self):
        prefix = self.install("core", "--core")
        self.verify(prefix, "hive-core")
        with self.daemon(prefix, "core") as (call, work, config, path):
            require(
                self.turn(call, work, "reply:installed-core") == "reply:installed-core",
                "Core turn output missing",
            )
        config["comb"] = {}
        write_json(path, config)
        rejected = run_process(
            call.argv("auth", "list"),
            text=True,
            capture_output=True,
            env=self.env,
            timeout=10,
            check=False,
        )
        require(rejected.returncode != 0, "Core accepted Comb configuration")
        error = json.loads(rejected.stdout)["error"]
        require("comb" in error["message"].lower(), error)
        return {"mock_turn": "completed", "comb_config": error}

    def hosted(self):
        prefix = self.install("hosted", "--comb")
        self.verify(prefix, "hive")
        self.verify(prefix, "comb")
        # Comb installs its bundled llama-server by default.
        self.verify(prefix, llama.PART)
        root = self.work / "knowledge"
        socket = self.work / "comb.sock"
        local = [str(prefix / "bin/comb"), "--root", str(root)]
        command = local + ["--socket", str(socket)]
        invoke = lambda *args: json_command(command + list(args), env=self.env)
        json_command(local + ["init"], env=self.env)
        with running(command + ["serve"], self.output / "comb-server.log", self.env) as server:
            wait_for(
                lambda: socket.exists() and ready(invoke, "stats"),
                "installed Comb server startup",
                server,
            )
            credential = self.work / "bridge.json"
            invoke(
                "credential",
                "create",
                "installed-runtime",
                "--bridge",
                "--reviewer-role",
                "implementor",
                "--out",
                str(credential),
            )
            additions = {
                "comb": {"credential_file": str(credential)},
                "comb_collectors": {
                    "shared": {"credential_file": str(credential), "sources": ["correction"]}
                },
            }
            with self.daemon(prefix, "hosted", additions) as (call, work, _, _):
                replies = self.hosted_tools(call, work)
                page = invoke("get", "concepts/installed")
                require("Release installation works." in json.dumps(page), page)
                admitted = call(
                    "evidence",
                    "correction",
                    "--summary",
                    "Installed collector delivered public correction.",
                    "--request-id",
                    "installed-correction",
                )

                def delivered():
                    status = call("collectors", "status")["collectors"]
                    return status and status[0]["pending"] == 0 and invoke("stats")["inbox"] == 1

                wait_for(delivered, "installed collector delivery", server)
                pages = json_command(
                    command + ["list", "--prefix", "inbox/"], env=self.env, allow_list=True
                )
                require(len(pages) == 1, pages)
                captured = invoke("get", pages[0]["slug"])
                require(
                    "Installed collector delivered public correction." in json.dumps(captured),
                    captured,
                )
                require(admitted["destinations"] == {"shared": "queued"}, admitted)
                graph = check_graph(invoke, lambda body: self.turn(call, work, body))
                return {
                    "hosted_tool_replies": replies,
                    "collector_admission": admitted,
                    "inbox": pages,
                    "graph": graph,
                }

    def hosted_tools(self, call, work):
        actions = [
            {
                "name": "comb_put",
                "arguments": {
                    "slug": "concepts/installed",
                    "markdown": "---\ntitle: Installed\n---\nRelease installation works.",
                },
            },
            {"name": "comb_search", "arguments": {"query": "installation", "mode": "text"}},
        ]
        output = self.turn(call, work, "comb:" + json.dumps(actions))
        require(output.startswith("comb-results:"), output)
        replies = json.loads(output.removeprefix("comb-results:"))
        require(len(replies) == 2 and all("error" not in r for r in replies), replies)
        require(
            replies[1]["mode"] == "text"
            and replies[1]["results"][0]["slug"] == "concepts/installed",
            replies[1],
        )
        return replies

    def wizard(self):
        """Exercise packaged setup and teardown in an isolated, manager-free home."""
        # Trusted launch images deliberately refuse HOME below /tmp.
        base = Path.home() / ".cache/lx-test"
        base.mkdir(parents=True, exist_ok=True, mode=0o700)
        with (
            private_directory(prefix="w-", parent=base) as directory,
            embedding_server() as url,
        ):
            home = Path(directory).resolve()
            runtime = home / "run"
            runtime.mkdir(mode=0o700)
            previous = self.env
            self.env = dict(
                previous,
                HOME=str(home),
                XDG_CONFIG_HOME=str(home / "cfg"),
                XDG_STATE_HOME=str(home / "st"),
                XDG_DATA_HOME=str(home / "data"),
                XDG_RUNTIME_DIR=str(runtime),
                DBUS_SESSION_BUS_ADDRESS="unix:path=" + str(runtime / "bus"),
            )
            try:
                return self._wizard(home, url)
            finally:
                self.env = previous

    def _wizard(self, home, url):
        return InstalledWizard(self, home, url).run()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    output = args.output_dir.resolve()
    create_directory(output)
    report = {"target": native_target(), "profile": "release", "checks": {}}
    # Keep sockets well within the Unix path limit, independently of checkout path.
    with (
        evidence(output / "diagnostics"),
        private_directory(prefix="hive-i-", parent="/tmp") as scratch,
    ):
        checks = InstalledChecks(args.assets.resolve(), output, Path(scratch).resolve())
        for name in ["core", "hosted", "wizard"]:
            began = time.monotonic()
            with phase("installed_" + name):
                report["checks"][name] = getattr(checks, name)()
            report["checks"][name]["seconds"] = time.monotonic() - began
            print(f"{name}: passed", flush=True)
    report["artifacts"] = checks.identities
    finish(lambda: (write_json(output / "results.json", report), print(output / "results.json")))


if __name__ == "__main__":
    raise SystemExit(entry(main, name="install"))
