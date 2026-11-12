#!/usr/bin/env python3
"""One owned live shop team: two Codex workers, a Claude reviewer and an approval.

Uses existing subscription logins and consumes quota. Owns its daemon, answers the
question with --answer, verifies independently, and ends all owned work on exit.
"""

import argparse
import json
import time
from pathlib import Path
from string import Template

from e2e.scripts.product_constants import LIVE_CLAUDE_MODEL
from e2e.scripts.harness.checks import require
from release.files import atomic_write_json, atomic_write_text
from e2e.live.scenario import Hosted, checked, options
from release.cancellation import pause
from release.process import run as run_process

PROJECT = Path(__file__).resolve().parent.parent
EXAMPLE = PROJECT / "examples/shop-team"
LEAD = "shop lead"
TEAM = {"restock fixer": "codex", "search builder": "codex", "claude reviewer": "claude"}
POLL_SECONDS = 5
# A coordinator group this long without activity, mail or a question cannot finish.
STALL_SECONDS = 60

# Independent behaviour checks of the finished repository, outside the agents' tests.
ORACLE = """import sys
from inventory import Inventory
from search import low_stock, search
inv = Inventory()
inv.add("Apple", 3, 0.5); inv.add("banana", 10, 0.25); inv.add("apricot", 1, 1.0)
inv.restock("Apple", 2)
if inv.items["Apple"]["quantity"] != 5: raise SystemExit("restock does not add")
for args in (("pear", 1), ("Apple", -1)):
    try: inv.restock(*args)
    except ValueError: pass
    else: raise SystemExit(f"restock accepted {args}")
if search(inv, "AP") != ["Apple", "apricot"]: raise SystemExit(f"search: {search(inv, 'AP')}")
if low_stock(inv, 5) != ["apricot"]: raise SystemExit(f"low_stock: {low_stock(inv, 5)}")
print("Independent checks passed: restock adds and refuses unknown or negative input; search and low_stock match.")
"""

# The baseline must still carry the restock bug, or the run proves nothing.
BASELINE = """from inventory import Inventory
inv = Inventory(); inv.add("a", 3, 1.0); inv.restock("a", 2)
if inv.items["a"]["quantity"] != 2: raise SystemExit("baseline restock already adds")
"""


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    options(parser, cap=1800)
    parser.add_argument("--answer", choices=("accept", "decline"), default="accept")
    parser.add_argument("--codex-auth", choices=("codex-main",), default="codex-main")
    parser.add_argument("--claude-auth", choices=("claude-main",), default="claude-main")
    parser.add_argument("--model", default="gpt-6.1-terra")
    parser.add_argument("--claude-model", default=LIVE_CLAUDE_MODEL)
    parser.add_argument("--coordinator-effort", default="medium")
    parser.add_argument("--worker-effort", default="medium")
    args = parser.parse_args()
    return checked(
        parser,
        args,
        label="shop-team",
        agents=4,
        models=dict(
            codex=args.model,
            codex_effort=args.worker_effort,
            claude=args.claude_model,
            claude_effort="low",
        ),
        cap=1800,
    )


def check(condition, reason):
    """Print a failed check as a result line first: the diagnostic entry point
    reports exception types and locations only, never their messages."""
    if not condition:
        print(json.dumps({"result": "failed", "reason": reason}), flush=True)
    require(condition, reason)


def git(repo, *arguments):
    run_process(
        ["git", *arguments], cwd=repo, check=True, capture_output=True, text=True, timeout=30
    )


def python(repo, *arguments):
    return run_process(
        ["python3", *arguments], cwd=repo, check=False, capture_output=True, text=True, timeout=30
    )


class ShopTeam(Hosted):
    def __init__(self, args):
        super().__init__(
            args,
            label="shop-team",
            roles={
                "shop-lead": dict(
                    auth=args.codex_auth,
                    model=args.model,
                    effort=args.coordinator_effort,
                    lifetime="independent",
                    sandbox="workspace-write",
                )
            },
        )
        self.call = self.cli

    def prerequisites(self):
        super().prerequisites()
        require(
            self.cli("auth", "status", self.args.claude_auth).get("state") == "authenticated",
            "An existing authenticated Claude subscription login is required",
        )

    def execute(self):
        manifest = self.start()
        self.wait(manifest, self.args.answer)
        return self.verify(manifest)

    def start(self):
        run, repo = self.output, self.work
        for source in sorted((EXAMPLE / "repo").iterdir()):
            atomic_write_text(repo / source.name, source.read_text())
        git(repo, "init", "-q")
        git(repo, "add", "-A")
        git(
            repo,
            "-c",
            "user.name=scenario",
            "-c",
            "user.email=scenario@example.invalid",
            "commit",
            "-qm",
            "Baseline stock tracker",
        )
        check(python(repo, "-m", "unittest").returncode == 0, "Baseline tests fail")
        check(python(repo, "-c", BASELINE).returncode == 0, "Baseline lost its restock bug")
        brief = run / "coordinator.md"
        atomic_write_text(
            brief,
            Template((EXAMPLE / "coordinator.md").read_text()).substitute(
                repo=repo,
                model=self.args.model,
                worker_effort=self.args.worker_effort,
                claude_auth=self.args.claude_auth,
                codex_auth=self.args.codex_auth,
                claude_model=self.args.claude_model,
            ),
        )
        reply = self.spawn_task("shop-lead", brief.read_text(), extra=("--name", LEAD))
        manifest = {
            "run": str(run),
            "repo": str(repo),
            "lead": reply["agent_id"],
            "started_at": int(time.time()),
        }
        atomic_write_json(run / "manifest.json", manifest)
        print(json.dumps(manifest), flush=True)
        return manifest

    def team(self, lead):
        return self.call("agents", "--parent", lead, "--limit", "200")["agents"]

    def stalled(self, lead):
        """The coordinator's whole group is quiet: nothing running, queued or asked."""
        after = []
        while True:
            page = self.call("agents", "--stalled", f"{STALL_SECONDS}s", "--limit", "100", *after)
            if any(group["root_id"] == lead for group in page["groups"]):
                return True
            if page.get("next_cursor") is None:
                return False
            after = ["--stalled-after", json.dumps(page["next_cursor"])]

    def wait(self, manifest, answer):
        lead = manifest["lead"]
        deadline = time.monotonic() + self.args.timeout
        seen = None
        while time.monotonic() < deadline:
            agent = self.call("status", lead)["agent"]
            rows = [(LEAD, agent["state"], agent["task_outcome"])]
            rows += [
                (row.get("name"), row["state"], row["task_outcome"]) for row in self.team(lead)
            ]
            if rows != seen:
                print(json.dumps({"agents": rows}), flush=True)
                seen = rows
            if agent.get("ended_at") is not None:
                return
            check(
                not self.stalled(lead),
                f"The run stalled: {LEAD} has been quiet for {STALL_SECONDS} s with its task "
                "open, and no agent is working, has mail or awaits an answer, so it cannot "
                "finish. Message it in hivectl tui to continue, or terminate it.",
            )
            if answer:
                for question in self.call("approvals", "--agent", lead)["approvals"]:
                    self.call(
                        "respond",
                        question["id"],
                        f"--{answer}",
                        "--request-id",
                        f"answer-{question['id']}",
                    )
                    print(json.dumps({"answered": question["title"], "action": answer}), flush=True)
            pause(POLL_SECONDS)
        check(False, f"The run did not finish within {self.args.timeout}s; owned cleanup follows.")

    def verify_selection(self, current, name):
        provider = "claude" if name == "claude reviewer" else "codex"
        expected = dict(
            provider=provider,
            auth=self.args.claude_auth if provider == "claude" else self.args.codex_auth,
            model=self.args.claude_model if provider == "claude" else self.args.model,
            effort="low"
            if provider == "claude"
            else self.args.coordinator_effort
            if name == LEAD
            else self.args.worker_effort,
        )
        check(
            all(current[key] == value for key, value in expected.items()),
            "Team selection differs from requested provider/auth/model/effort",
        )
        stream = self.events(current["id"])
        ready = [event["data"] for event in stream if event["type"] == "session_ready"]
        check(
            bool(ready)
            and all(
                item["model"] == expected["model"] and item["effort"] == expected["effort"]
                for item in ready
            ),
            "Actual team model/effort differs from requested selection",
        )
        check(
            not any(event["type"] in ("execution_failed", "model_rerouted") for event in stream),
            "Team provider failed or rerouted",
        )

    def verify(self, manifest):
        lead, repo, run = manifest["lead"], Path(manifest["repo"]), Path(manifest["run"])
        agent = self.call("status", lead)["agent"]
        check(agent.get("name") == LEAD, f"Coordinator name: {agent.get('name')}")
        check(agent["task_outcome"] == "completed", f"Coordinator outcome: {agent['task_outcome']}")
        team = {row.get("name"): row for row in self.team(lead)}
        check(set(team) == set(TEAM), f"Team names: {sorted(map(str, team))}")
        for name, provider in TEAM.items():
            check(team[name]["provider"] == provider, f"{name} provider: {team[name]['provider']}")
        reviewer = self.call("status", team["claude reviewer"]["id"])["agent"]
        check(reviewer["sandbox"] == "read-only", f"Reviewer sandbox: {reviewer['sandbox']}")
        check(
            reviewer["agent_options"]["options"].get("claude.skills") is True,
            "Reviewer skills are off",
        )
        self.verify_selection(agent, LEAD)
        for name, current in team.items():
            self.verify_selection(current, name)
        kinds = [event["type"] for event in self.events(lead)]
        check(
            kinds.count("completion_requested") == 1,
            f"hive_complete calls: {kinds.count('completion_requested')}",
        )
        check("join_completed" in kinds, "The workers' joined replies never arrived")
        questions = self.call("approvals", "--all", "--agent", lead)["approvals"]
        answered = [q for q in questions if q["state"] == "answered"]
        check(len(answered) == 1, f"Answered questions: {len(answered)} of {len(questions)}")
        action = answered[0]["action"]
        tests = python(repo, "-m", "unittest")
        atomic_write_json(
            run / "repository-tests.json",
            dict(returncode=tests.returncode, stdout=tests.stdout, stderr=tests.stderr),
        )
        check(tests.returncode == 0, "Repository tests fail; inspect retained native output")
        oracle = python(repo, "-c", ORACLE)
        atomic_write_json(
            run / "oracle.json",
            dict(returncode=oracle.returncode, stdout=oracle.stdout, stderr=oracle.stderr),
        )
        check(oracle.returncode == 0, "Independent checks fail; inspect retained native output")
        changelog = (repo / "CHANGELOG.md").is_file()
        check(changelog == (action == "accept"), f"CHANGELOG.md present {changelog} after {action}")
        result = {
            "result": "passed",
            "lead": lead,
            "answer": action,
            "team": {name: row["id"] for name, row in team.items()},
            "checks": oracle.stdout.strip(),
        }
        atomic_write_json(run / "observations.json", result)
        return result


def main():
    return ShopTeam(parse_args()).run()


if __name__ == "__main__":
    from release.progress import entry

    raise SystemExit(entry(main, name="scenario"))
