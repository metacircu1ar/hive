#!/usr/bin/env python3
"""Run the live Hive review scenario using an existing subscription login.

This runs model turns. It does not log in, read credentials, or stop an existing daemon.
Owns its daemon and terminates all owned work after pass, failure or timeout.
"""

import argparse
import hashlib
import json
import os
import re
import sys
import time
from pathlib import Path

from e2e.live.scenario import Hosted, checked, options
from release.cancellation import pause
from release.process import run as run_process
from release.files import atomic_write_json

from e2e.scripts.harness.checks import require
from e2e.scripts.harness.cli import read_agents

CHECKER = """import importlib.util, math, pathlib
def load(path):
    spec=importlib.util.spec_from_file_location("check_factorial",path)
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module);return module.factorial
f=load("factorial.py")
for n in range(201):
    if f(n)!=math.factorial(n): raise RuntimeError(n)
for n in [-1,-2,-100]:
    try: f(n)
    except ValueError: pass
    else: raise AssertionError(("negative accepted",n))
for n in [True,False,1.0,1.5,"5",None,[],{},complex(1,0)]:
    try: f(n)
    except TypeError: pass
    else: raise AssertionError(("non-integer accepted",n))
initial=load("evidence/initial_factorial.py")
if initial(5)==math.factorial(5): raise RuntimeError("Initial snapshot was not faulty")
print("Independent checks passed: 201 factorial values, 3 negatives, 9 invalid types; faulty initial snapshot confirmed.")
"""


def parse_args():
    project = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    options(parser, cap=1200)
    parser.add_argument("--expected-initial-sha256")
    parser.add_argument("--require-resumed-tool", action="store_true")
    parser.add_argument("--expected-model", default="gpt-6.1-terra")
    parser.add_argument("--expected-effort", default="medium")
    args = parser.parse_args()
    if (
        args.expected_initial_sha256
        and re.fullmatch("[0-9a-f]{64}", args.expected_initial_sha256) is None
    ):
        parser.error("--expected-initial-sha256 must be 64 lowercase hexadecimal characters")
    checked(
        parser,
        args,
        label="factorial",
        agents=2,
        models=dict(model=args.expected_model, effort=args.expected_effort),
        cap=1200,
    )
    return args, project, parser


class FactorialRun(Hosted):
    """Collect and independently verify one owned live review scenario."""

    def __init__(self, args, project, parser):
        self.project, self.parser = project, parser
        roles = {
            name: dict(
                auth="codex-main",
                model=args.expected_model,
                effort=args.expected_effort,
                instructions=str(project / "examples/roles" / (name + ".md")),
                lifetime="independent" if name == "implementor" else "with_parent",
                sandbox="workspace-write" if name == "implementor" else "read-only",
            )
            for name in ("implementor", "reviewer")
        }
        super().__init__(args, label="factorial", roles=roles)
        self.call = self.cli
        self.repo = self.work

    def prepare(self):
        run_process(["git", "init", "--quiet", str(self.repo)], check=True, timeout=10)

    def execute(self):
        self.agent_id = self.spawn_task(
            "implementor", (self.project / "examples/factorial-task.md").read_text()
        )["agent_id"]
        print(json.dumps(dict(repo=str(self.repo), implementor=self.agent_id)), flush=True)
        self.wait()
        self.review_exchange()
        self.check_source()
        self.collect_events()
        return self.report()

    def wait(self):
        deadline = time.monotonic() + self.args.timeout
        last_state = None
        while time.monotonic() < deadline:
            self.agent = self.call("status", self.agent_id)["agent"]
            state = (self.agent["state"], self.agent["task_outcome"])
            if state != last_state:
                print(json.dumps({"state": state[0], "task_outcome": state[1]}), flush=True)
                last_state = state
            if self.agent["state"] == "failed":
                raise RuntimeError({"agent_failed": self.agent["error"], "agent_id": self.agent_id})
            if self.agent["task_outcome"] == "completed" and self.agent["state"] == "stopped":
                break
            pause(0.5)
        else:
            raise RuntimeError("Factorial scenario timed out; owned cleanup follows.")

    def review_exchange(self):
        agents = read_agents(self.call)
        children = [a for a in agents if a["parent_id"] == self.agent_id]
        if not (len(children) == 1):
            require(False, "Expected exactly one reviewer")
        self.reviewer = children[0]
        if self.args.expected_model:
            if not (self.agent["model"] == self.reviewer["model"] == self.args.expected_model):
                require(
                    False,
                    'Factorial scenario check failed: agent["model"] == reviewer["model"] == args.expected_model',
                )
        else:
            if not (self.agent["model"] != self.reviewer["model"]):
                require(
                    False, 'Factorial scenario check failed: agent["model"] != reviewer["model"]'
                )
        if not (self.agent["effort"] == self.reviewer["effort"] == self.args.expected_effort):
            require(
                False,
                'Factorial scenario check failed: agent["effort"] == reviewer["effort"] == args.expected_effort',
            )
        if not (self.agent["thread_id"] != self.reviewer["thread_id"]):
            require(
                False,
                'Factorial scenario check failed: agent["thread_id"] != reviewer["thread_id"]',
            )
        self.messages = self.call("messages", self.agent_id)["messages"]
        reviews = [
            m
            for m in self.messages
            if m["sender"] == self.reviewer["id"] and m["recipient"] == self.agent_id
        ]
        if not (len(reviews) >= 2):
            require(False, "Expected findings followed by a clean review")
        # The messages endpoint is ordered newest first. Only delivered feedback counts.
        if not (reviews[0]["body"].lstrip().startswith("NO_FINDINGS")):
            require(False, "Latest reviewer feedback is not clean")
        if not (reviews[0]["state"] == "completed"):
            require(False, "Clean feedback was not processed by the implementor")
        if not (
            any(
                m["state"] == "completed" and not m["body"].lstrip().startswith("NO_FINDINGS")
                for m in reviews[1:]
            )
        ):
            require(False, "No earlier findings were processed")

    def check_source(self):
        initial = self.repo / "evidence/initial_factorial.py"
        self.initial_hash = hashlib.sha256(initial.read_bytes()).hexdigest()
        if self.args.expected_initial_sha256:
            if not (self.initial_hash == self.args.expected_initial_sha256):
                require(False, "Initial source changed since the checkpoint")
        if self.initial_hash not in (self.repo / "evidence/initial.sha256").read_text():
            require(
                False,
                'Factorial scenario check failed: initial_hash in (repo / "evidence/initial.sha256").read_text()',
            )
        if not (initial.read_bytes() != (self.repo / "factorial.py").read_bytes()):
            require(
                False,
                'Factorial scenario check failed: initial.read_bytes() != (repo / "factorial.py").read_bytes()',
            )
        env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
        self.independent = run_process(
            [sys.executable, "-c", CHECKER],
            cwd=self.repo,
            env=env,
            capture_output=True,
            text=True,
            check=True,
            timeout=20,
        )
        self.tests = run_process(
            [sys.executable, "-m", "unittest", "discover", "-v"],
            cwd=self.repo,
            env=env,
            capture_output=True,
            text=True,
            check=True,
            timeout=30,
        )

    def collect_events(self):
        self.event_streams = {}
        for a in [self.agent, self.reviewer]:
            stream = self.events(a["id"])
            self.event_streams[a["id"]] = stream
            require(
                not any(e["type"] in ("execution_failed", "model_rerouted") for e in stream),
                "Factorial provider failed or rerouted",
            )
            ready = [e for e in stream if e["type"] == "session_ready"]
            if not (
                ready
                and all(
                    e["data"]["model"] == a["model"]
                    and e["data"]["effort"] == self.args.expected_effort
                    for e in ready
                )
            ):
                require(
                    False,
                    'Factorial scenario check failed: ready and all(\n            e["data"]["model"] == a["model"] and e["data"]["effort"] == args.expected_effort\n            for e in ready\n        )',
                )
            if self.args.require_resumed_tool:
                resumed = [e for e in ready if e["data"].get("resumed") is True]
                if not (resumed):
                    require(False, f"No resumed session for {a['id']}")
                if not (
                    any(
                        e["type"] == "tool_completed"
                        and e["data"].get("item", {}).get("type") == "dynamicToolCall"
                        and e["data"]["item"].get("status") == "completed"
                        and e["data"]["item"].get("success") is True
                        and e["data"]["item"].get("tool", "").startswith("hive_")
                        and any(
                            e["seq"] > r["seq"] and e["execution_id"] == r["execution_id"]
                            for r in resumed
                        )
                        for e in stream
                    )
                ):
                    require(False, f"No successful Hive tool after resume for {a['id']}")

    def report(self):
        evidence = {
            "repo": str(self.repo),
            "implementor": self.agent,
            "reviewer": self.reviewer,
            "initial_sha256": self.initial_hash,
            "messages": self.messages,
            "events": self.event_streams,
            "independent_checks": self.independent.stdout,
            "unittest": self.tests.stdout + self.tests.stderr,
        }
        atomic_write_json(self.output / "observations.json", evidence)
        return dict(
            initial_sha256=self.initial_hash,
            reviewer=self.reviewer["id"],
            independent_checks=self.independent.stdout,
            unittest=self.tests.stdout + self.tests.stderr,
        )


def main():
    args, project, parser = parse_args()
    return FactorialRun(args, project, parser).run()


if __name__ == "__main__":
    from release.progress import entry

    raise SystemExit(entry(main, name="scenario"))
