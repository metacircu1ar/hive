#!/usr/bin/env python3
"""Live model/history, redirect and Claude steering checks. Uses subscription model turns.

Requires existing subscription logins, an otherwise unused controller and a stopped daemon. Runs only a release
binary, never copies credentials, and retains reports/workspaces on failure.
Run verify-steering for Sonnet low before the Claude cases.
"""

from release.files import atomic_write_text

import argparse
import json
import time
import uuid

from e2e.live.scenario import Hosted, checked, options
from e2e.live.ownership import preflight as state_preflight
from release.cancellation import pause, bounded
from release.process import run as run_process

from e2e.scripts.harness.checks import require
from release.progress import finalizer, cleanup_all
from e2e.scripts.harness.cli import collect_events
from e2e.scripts.harness.daemon import control_request
from e2e.scripts.product_constants import LIVE_CODEX_EFFORT, LIVE_CODEX_MODEL
from e2e.scripts.verify_ping_pong_pair import verify_cleanup_observation

GATE = """import json, os, pathlib, subprocess, sys, time
name = sys.argv[1]
child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(180)'])
ready = pathlib.Path(name + '.ready')
temporary = ready.with_suffix('.tmp')
temporary.write_text(json.dumps({'parent':os.getpid(), 'child':child.pid}))
temporary.replace(ready)
try:
    deadline = time.monotonic() + 180
    while not pathlib.Path(name + '.release').exists():
        if time.monotonic() >= deadline:
            raise TimeoutError('scenario gate expired')
        time.sleep(.02)
finally:
    child.terminate()
    child.wait(timeout=5)
print('gate finished')
"""


class ControlError(RuntimeError):
    def __init__(self, response):
        self.response = response
        super().__init__(response)


def assert_clean_executions(events):
    require(
        not any(e["type"] in ("session_quarantined", "execution_failed") for e in events),
        "Provider execution failed",
    )
    starts = [e for e in events if e["type"] == "provider_started"]
    require(len(starts) == len({e["execution_id"] for e in starts}), "Duplicate provider start")
    known = {e["execution_id"] for e in starts}
    endings = [e for e in events if e["type"] == "execution_stopped"]
    require(known == {e["execution_id"] for e in endings}, "Missing execution ending evidence")
    releases = [
        e for e in events if e["type"] in ("provider_session_released", "provider_shutdown")
    ]
    require(
        known == {e["execution_id"] for e in releases}, "Missing provider start/release evidence"
    )
    for start in starts:
        execution = start["execution_id"]
        session = [
            e
            for e in releases
            if e["execution_id"] == execution and e["type"] == "provider_session_released"
        ]
        physical = [
            e
            for e in releases
            if e["execution_id"] == execution and e["type"] == "provider_shutdown"
        ]
        require(len(session) <= 1 and len(physical) <= 1, "Duplicate provider release")
        selected = session or physical
        require(len(selected) == 1, "Missing provider start/release evidence")
        release = selected[0]
        stopped = [e for e in endings if e["execution_id"] == execution]
        require(len(stopped) == 1, "Duplicate execution ending")
        require(
            release["data"]["confirmed"] is True and release["data"]["quiescent"] is True,
            "Provider cleanup unconfirmed",
        )
        require(
            start["seq"] < release["seq"] < stopped[0]["seq"], "Invalid execution release order"
        )


class Run(Hosted):
    def __init__(self, args):
        instructions = args.output_dir.resolve() / "instructions.md"
        roles = {
            provider: dict(
                auth=provider + "-main",
                model=model,
                effort="low" if provider == "claude" else LIVE_CODEX_EFFORT,
                instructions=str(instructions),
                lifetime="independent",
                sandbox="workspace-write",
            )
            for provider, model in (("codex", args.codex_model), ("claude", args.claude_model))
            if args.provider in ("both", provider)
        }
        super().__init__(args, label="controls", roles=roles)
        atomic_write_text(
            instructions,
            "Follow the current request exactly. Do not delegate, use native "
            "subagents or approvals, or complete your task. Use tools only when requested. Keep answers short.\n",
        )
        self.root, self.workspace = args.state_dir, self.work
        self.agents = self.roots
        self.spawn_intents = []
        self.cleanup_until = None
        self.results = dict(
            workspace=str(self.workspace),
            state_dir=str(self.root),
            build_profile="release",
            providers={},
            provider_scope=args.provider,
            cleanup_errors=[],
            gates={},
            inconclusive=[],
        )

    def prepare(self):
        run_process(["git", "init", "--quiet", str(self.workspace)], check=True, timeout=10)
        atomic_write_text(self.workspace / "gate.py", GATE)
        atomic_write_text(self.workspace / "fixed.txt", "A harmless tool boundary.\n")

    def save(self):
        atomic_write_text(self.output / "observations.json", json.dumps(self.results, indent=2))

    def preflight(self):
        return state_preflight(self.root)

    def spawn(self, **args):
        intent = dict(request_id=str(uuid.uuid4()), args=args, agent=None)
        self.spawn_intents.append(intent)
        self.pending[intent["request_id"]] = intent
        self.results["spawn_intents"] = self.spawn_intents
        self.save()  # Preserve the exact id/envelope before acceptance can be ambiguous.
        return self.resolve_spawn(intent)

    def resolve_spawn(self, intent):
        result = self.call("spawn", request_id=intent["request_id"], **intent["args"])
        agent = result["agent_id"]
        intent["agent"] = agent
        if agent not in self.agents:
            self.agents.append(agent)
        self.pending.pop(intent["request_id"], None)
        self.save()
        return agent

    def call(self, op, request_id=None, **args):
        request = dict(protocol=1, id=request_id or str(uuid.uuid4()), args=dict(op=op, **args))
        timeout = bounded(45)
        if self.cleanup_until is not None:
            timeout = min(timeout, self.cleanup_until - time.monotonic())
        if timeout <= 0:
            raise TimeoutError("Controls work or cleanup budget expired")
        value = control_request(self.root / "runtime/control.sock", request, timeout=timeout)
        if "error" in value:
            raise ControlError(value)
        return value

    def wait(self, check, description, timeout=180):
        deadline = time.monotonic() + bounded(timeout)
        if self.cleanup_until is not None:
            deadline = min(deadline, self.cleanup_until)
        while time.monotonic() < deadline:
            value = check()
            if value:
                return value
            pause(0.05)
        raise TimeoutError(description)

    def status(self, agent):
        value = self.call("status", target=agent)["agent"]
        if value["state"] == "failed":
            raise RuntimeError(value)
        return value

    def events(self, agent):
        return collect_events(
            lambda after: self.call("read", target=agent, after=after, limit=200), max_pages=100
        )

    def messages(self, agent):
        messages, before = [], None
        for _ in range(10):
            args = {} if before is None else dict(before=before)
            page = self.call("messages", target=agent, limit=100, **args)
            if not page["messages"]:
                return messages
            messages.extend(page["messages"])
            cursor = page["next_before"]
            if cursor is None or (before is not None and cursor >= before):
                raise RuntimeError("Mailbox cursor did not advance")
            before = cursor
        raise RuntimeError("Unexpected scenario mailbox size")

    def delivered(self, agent, message, state="completed"):
        def check():
            self.status(agent)
            row = next(m for m in self.messages(agent) if m["id"] == message)
            if row["state"] in ("failed", "uncertain", "undeliverable"):
                raise RuntimeError(row)
            if row["state"] not in ("accepted", "submitting", "submitted", state):
                raise RuntimeError(
                    f"Unexpected terminal message state: {row['state']}; expected {state}"
                )
            return row if row["state"] == state else None

        return self.wait(check, f"{message}: waiting for {state}")

    def turn(self, agent, message):
        matches = [
            e["data"]
            for e in self.events(agent)
            if e["type"] == "turn_started" and e["data"]["message_id"] == message
        ]
        if not (len(matches) == 1):
            require(False, matches)
        return matches[0]

    def text(self, agent, message):
        turn = self.turn(agent, message)["turn_id"]
        return "\n".join(
            e["data"]["text"]
            for e in self.events(agent)
            if e["type"] == "assistant_message" and e["data"]["turn_id"] == turn
        )

    def send(self, agent, body, mode="next-turn"):
        return self.call("send", target=agent, body=body, mode=mode)["message_id"]

    def steer(self, agent, active, body):
        response = self.call("send", target=agent, body=body, mode="steer")
        require(
            response["steering_target"]
            == dict(execution_id=active["execution_id"], turn_id=active["turn_id"]),
            "Steering target changed",
        )
        return response

    def remember(self, agent, marker):
        message = self.send(
            agent, "Reply only with the secret marker from the first message. Use no tools."
        )
        self.delivered(agent, message)
        require(marker in self.text(agent, message), "Conversation recall failed")
        turn = self.turn(agent, message)
        require(
            not any(
                e["type"] == "tool_completed" and e["data"]["turn_id"] == turn["turn_id"]
                for e in self.events(agent)
            ),
            "Recall used tools",
        )
        return turn

    def resume(self, agent, thread, marker):
        previous = self.status(agent)["execution_id"]
        self.call("stop", target=agent)
        self.wait(lambda: self.status(agent)["state"] == "stopped", "agent stopped")
        self.call("resume", target=agent)
        turn = self.remember(agent, marker)
        now = self.status(agent)
        require(
            now["thread_id"] == thread and now["execution_id"] != previous,
            "Resume changed conversation or retained execution",
        )
        return turn

    def gate(self, agent, name, reads=False):
        print(json.dumps(dict(stage="gate", agent=agent, name=name)), flush=True)
        command = f"python3 gate.py {name}"
        body = (
            "This is an authorized Hive integration test of a local helper that waits "
            "for the test runner. "
            f"Run exactly `{command}` using the shell tool and wait for it to finish. "
        )
        if reads:
            body += "Then read fixed.txt three times using separate Read calls. "
        body += "Then give a brief final answer. Use no other tools."
        message = self.send(agent, body)
        ready = self.workspace / (name + ".ready")

        def check():
            self.status(agent)
            if ready.exists():
                return json.loads(ready.read_text())
            row = next(m for m in self.messages(agent) if m["id"] == message)
            if row["state"] not in ("accepted", "submitting", "submitted"):
                raise RuntimeError(f"Gate turn ended before helper readiness: {row['state']}")
            return None

        pids = self.wait(check, "native shell helper ready")
        identity = {}
        for key, pid in pids.items():
            result = run_process(
                ["ps", "-p", str(pid), "-o", "lstart="],
                capture_output=True,
                text=True,
                check=True,
                timeout=5,
            )
            require(result.stdout.strip(), "Missing helper process identity")
            identity[key] = dict(pid=pid, birth=result.stdout.strip())
            self.results["gates"][name] = identity
            self.save()
        active = self.wait(
            lambda: a if (a := self.status(agent))["turn_id"] else None, "active gate turn"
        )
        self.delivered(agent, message, "submitted")
        return message, active, identity

    def gone(self, identities):
        until = time.monotonic() + bounded(30)
        if self.cleanup_until is not None:
            until = min(until, self.cleanup_until)

        def check():
            for identity in identities.values():
                left = min(5, until - time.monotonic())
                if left <= 0:
                    raise TimeoutError("Helper absence observation budget expired")
                result = run_process(
                    ["ps", "-p", str(identity["pid"]), "-o", "lstart="],
                    capture_output=True,
                    text=True,
                    timeout=left,
                    check=False,
                )
                require(result.returncode in (0, 1), "Cannot inspect helper process")
                if result.stdout.strip() == identity["birth"]:
                    return False
            return True

        self.wait(check, "owned shell helper gone", timeout=max(0, until - time.monotonic()))

    def change(self, agent, model, effort):
        request = str(uuid.uuid4())
        args = dict(target=agent, options=dict(model=model, effort=effort))
        first = self.call("set_options", request_id=request, **args)
        require(
            self.call("set_options", request_id=request, **args) == first,
            "Model change retry changed response",
        )
        return first

    def terminal_cleanup_observed(self, agent, active):
        def check():
            events = [
                e
                for e in self.events(agent)
                if e["type"] == "terminal_cleanup_observed"
                and e["execution_id"] == active["execution_id"]
                and e["data"]["turn_id"] == active["turn_id"]
            ]
            require(len(events) <= 1, "Duplicate terminal cleanup observation")
            return events[0] if events else None

        event = self.wait(check, "durable bounded terminal cleanup observation", timeout=25)
        require(
            event["data"]["remaining"] == 0 and event["data"]["next_cursor"] is None,
            "Terminal cleanup retained pending work",
        )
        observation = event["data"]["observation"]
        verify_cleanup_observation(observation)
        finished = next(
            e
            for e in self.events(agent)
            if e["type"] == "turn_finished"
            and e["execution_id"] == active["execution_id"]
            and e["data"]["turn_id"] == active["turn_id"]
        )
        require(finished["seq"] < event["seq"], "Terminal cleanup predates turn finish")
        return event

    def interrupt_trials(self, agent, thread, model):
        trials = self.results.setdefault("codex_interrupt_trials", [])
        for i in range(3):
            message, active, identities = self.gate(agent, "bounded-interrupt-" + str(i))
            trial = dict(
                message=message,
                model=model,
                effort=LIVE_CODEX_EFFORT,
                turn=active["turn_id"],
                helper_identities=identities,
                passed=False,
            )
            trials.append(trial)
            self.save()
            self.call("interrupt", target=agent)
            self.delivered(agent, message, "interrupted")
            trial["cleanup"] = self.terminal_cleanup_observed(agent, active)
            self.gone(identities)  # Check after the window, not just after interrupt ACK.
            trial["helpers_absent_after_window"] = True
            followup = self.send(agent, "Reply NEXT. Use no tools.")
            self.delivered(agent, followup)
            require(self.status(agent)["thread_id"] == thread, "Interrupt changed conversation")
            failures = [e for e in self.events(agent) if e["type"] == "session_quarantined"]
            trial["post_window_failures"] = failures
            if not (not failures):
                require(False, failures)
            trial["passed"] = True
            self.save()
            print(
                json.dumps(
                    dict(
                        stage="codex_interrupt_trial_passed",
                        trial=i + 1,
                        observation=trial["cleanup"]["data"]["observation"],
                    )
                ),
                flush=True,
            )

    def provider(self, kind, model, changed, effort):
        marker = str(uuid.uuid4())  # Never written inside the agent's workspace.
        agent = self.spawn(
            role=kind,
            cwd=str(self.workspace),
            body=f"Remember this secret marker: {marker}. Reply READY. Use no tools.",
        )
        print(json.dumps(dict(stage="agent_spawned", provider=kind, agent=agent)), flush=True)
        first = self.messages(agent)[0]["id"]
        self.delivered(agent, first)
        thread = self.status(agent)["thread_id"]
        if kind == "codex":
            self.interrupt_trials(agent, thread, model)
        change = self.change(agent, changed, effort)
        recalled = self.remember(agent, marker)
        require(
            recalled["model"] == changed and recalled["effort"] == effort,
            "Recall changed requested model or effort",
        )
        require(
            recalled["options_revision"] == change["desired"]["revision"],
            "Recall used wrong options revision",
        )
        require(self.status(agent)["thread_id"] == thread, "Model change changed conversation")
        if kind == "codex":
            restored = self.change(agent, model, LIVE_CODEX_EFFORT)
            selected = self.remember(agent, marker)
            require(
                selected["model"] == model
                and selected["effort"] == LIVE_CODEX_EFFORT
                and selected["options_revision"] == restored["desired"]["revision"],
                "Recall did not restore the initial Codex selection",
            )
            require(self.status(agent)["thread_id"] == thread, "Restoring model changed conversation")
        resumed = self.resume(agent, thread, marker)
        message, active, pids = self.gate(agent, kind + "-interrupt")
        self.call("interrupt", target=agent)
        self.delivered(agent, message, "interrupted")
        cleaned = None
        if kind == "codex":
            cleaned = self.terminal_cleanup_observed(agent, active)
        self.gone(pids)
        resumed_after_interrupt = self.resume(agent, thread, marker)
        batch, pids, cleaned = self.redirect(agent, thread, kind, cleaned)
        if kind == "claude":
            self.change(agent, model, "low")
            self.steering(agent)
            self.no_tool_redirect(agent, thread)
        self.results["providers"][kind] = dict(
            agent=agent,
            thread=thread,
            initial_model=model,
            changed_model=changed,
            changed_effort=effort,
            completed_resume=resumed,
            interrupted_resume=resumed_after_interrupt,
            redirect=batch,
            helper_identities=pids,
            redirect_cleanup=cleaned,
        )
        self.save()

    def no_tool_redirect(self, agent, thread):
        message = self.send(
            agent, "Use no tools. Write a 1000-word description of a quiet mountain lake."
        )

        def observe():
            status = self.status(agent)
            row = next(m for m in self.messages(agent) if m["id"] == message)
            return (status, row) if status["turn_id"] or row["state"] == "completed" else None

        active, row = self.wait(observe, "no-tool redirect turn active or completed")
        if row["state"] == "completed":
            self.results["inconclusive"].append(
                "No-tool redirect turn ended before guidance acceptance"
            )
            return
        marker, request = str(uuid.uuid4()), str(uuid.uuid4())
        args = dict(
            target=agent,
            body=f"Reply only {marker}. Use no tools.",
            mode="redirect",
            request_id=request,
        )
        accepted = self.call("send", **args)
        self.delivered(agent, accepted["message_id"])
        require(self.call("send", **args) == accepted, "No-tool redirect retry changed response")
        row = next(m for m in self.messages(agent) if m["id"] == message)
        self.results["no_tool_redirect"] = dict(acceptance=accepted, original_state=row["state"])
        if row["state"] == "completed":
            self.results["inconclusive"].append(
                "No-tool turn finished before redirect interruption"
            )
        else:
            if not (row["state"] == "interrupted"):
                require(False, row)
            require(
                accepted["redirect_target"]
                == dict(
                    execution_id=active["execution_id"],
                    turn_id=active["turn_id"],
                    message_id=message,
                ),
                "No-tool redirect target changed",
            )
        events = self.events(agent)
        require(
            not any(
                e["type"] == "tool_completed" and e["data"]["turn_id"] == active["turn_id"]
                for e in events
            ),
            "No-tool redirect used tools",
        )
        require(
            sum(
                e["type"] == "turn_started" and e["data"]["message_id"] == accepted["message_id"]
                for e in events
            )
            == 1,
            "No-tool redirect delivered more than once",
        )
        require(
            marker in self.text(agent, accepted["message_id"]),
            "No-tool redirect marker missing from output",
        )
        require(self.status(agent)["thread_id"] == thread, "No-tool redirect changed conversation")
        self.save()

    def steering(self, agent):
        message, active, pids = self.gate(agent, "claude-steer", reads=True)
        require(
            self.status(agent).get("steering_supported"),
            "Run native verify-steering for this selection first",
        )
        markers = [str(uuid.uuid4()) for _ in range(3)]
        operations = [
            dict(
                op="send",
                target=agent,
                mode="steer",
                request_id=str(uuid.uuid4()),
                body=f"Include marker {marker} in your final answer in the order guidance arrives.",
            )
            for marker in markers
        ]
        batch = self.call("batch", operations=operations)["results"]
        target = dict(execution_id=active["execution_id"], turn_id=active["turn_id"])
        if not (all(r.get("steering_target") == target for r in batch)):
            require(False, batch)
        for row in batch:
            self.delivered(agent, row["message_id"], "submitting")
        proof = json.loads((self.root / "runtime/claude-steering-proof.json").read_text())
        self.results["steering_proof"] = proof
        receipt_mode = proof["proof"]["receipt_mode"]
        # More Read boundaries follow the gate. Receipt evidence determines the
        # batches actually delivered; acceptance alone is not a codec barrier.
        atomic_write_text(self.workspace / "claude-steer.release", "")
        self.delivered(agent, message)
        for row in batch:
            self.delivered(agent, row["message_id"])
        receipts = self.steering_receipts(agent, active, receipt_mode)
        combined = "\n".join(receipts)
        if not (all(combined.count(marker) == 1 for marker in markers)):
            require(False, combined)
        require(
            [combined.index(m) for m in markers] == sorted(combined.index(m) for m in markers),
            "Steering receipt marker order changed",
        )
        text = self.text(agent, message)
        if not (all(text.count(marker) == 1 for marker in markers)):
            require(False, text)
        self.results["steering"] = dict(
            receipt_mode=receipt_mode,
            accepted=batch,
            output_marker_order=sorted(range(len(markers)), key=lambda i: text.index(markers[i])),
        )
        self.gone(pids)
        self.held_steering(agent)
        self.no_tool_steering(agent)

    def assert_stale(self, agent, message, guidance, marker):
        self.delivered(agent, guidance["message_id"], "stale_turn")
        require(
            marker not in self.text(agent, message), "Stale guidance leaked into original output"
        )
        require(
            not any(
                e["type"] == "steering_submitted"
                and e["data"]["message_id"] == guidance["message_id"]
                for e in self.events(agent)
            ),
            "Stale guidance has successful receipt",
        )
        following = self.send(agent, "Reply only NEXT. Use no tools.")
        self.delivered(agent, following)
        require(
            marker not in self.text(agent, following), "Stale guidance leaked into later output"
        )

    def execute(self):
        for kind, model, changed, effort in (
            (
                "codex",
                self.args.codex_model,
                self.args.codex_changed_model,
                self.args.codex_changed_effort,
            ),
            (
                "claude",
                self.args.claude_model,
                self.args.claude_changed_model,
                self.args.claude_changed_effort,
            ),
        ):
            if self.args.provider in ("both", kind):
                self.provider(kind, model, changed, effort)
        require(not self.results["inconclusive"], "A control race check was inconclusive")
        return self.results

    def redirect(self, agent, thread, kind, cleaned):
        message, active, pids = self.gate(agent, kind + "-redirect")
        request = str(uuid.uuid4())
        ordinary_marker, redirect_marker = str(uuid.uuid4()), str(uuid.uuid4())
        operations = [
            dict(
                op="send",
                target=agent,
                body=f"Reply only {ordinary_marker}. Use no tools.",
                request_id=request + "-ordinary",
            ),
            dict(
                op="send",
                target=agent,
                body=f"Reply only {redirect_marker}. Use no tools.",
                mode="redirect",
                request_id=request + "-redirect",
            ),
        ]
        batch = self.call("batch", request_id=request, operations=operations)
        if not (not any("error" in row for row in batch["results"])):
            require(False, batch)
        ordinary, redirect = batch["results"]
        require(
            redirect["redirect_target"]["turn_id"] == active["turn_id"],
            "Redirect targeted wrong turn",
        )
        self.delivered(agent, message, "interrupted")
        self.delivered(agent, redirect["message_id"])
        self.delivered(agent, ordinary["message_id"])
        self.gone(pids)
        if kind == "codex":
            self.terminal_cleanup_observed(agent, active)
        retry = self.call("batch", request_id=request, operations=operations)
        require(
            retry["results"][1]["message_id"] == redirect["message_id"],
            "Redirect retry changed message",
        )
        require(
            retry["results"][1]["redirect_target"] == redirect["redirect_target"],
            "Redirect retry changed target",
        )
        starts = [e for e in self.events(agent) if e["type"] == "turn_started"]
        order = [e["data"]["message_id"] for e in starts]
        require(
            order.count(redirect["message_id"]) == order.count(ordinary["message_id"]) == 1,
            "Redirect or ordinary input delivered more than once",
        )
        require(
            order.index(redirect["message_id"]) < order.index(ordinary["message_id"]),
            "Redirect did not precede ordinary input",
        )
        if kind == "codex":
            cleaned = self.terminal_cleanup_observed(agent, active)
            redirected = next(
                e for e in starts if e["data"]["message_id"] == redirect["message_id"]
            )
            require(cleaned["seq"] < redirected["seq"], "Redirect admitted before terminal cleanup")
        require(
            redirect_marker in self.text(agent, redirect["message_id"]),
            "Redirect marker missing from output",
        )
        require(
            ordinary_marker in self.text(agent, ordinary["message_id"]),
            "Ordinary marker missing from output",
        )
        require(self.status(agent)["thread_id"] == thread, "Redirect changed conversation")
        return batch, pids, cleaned

    def no_tool_steering(self, agent):
        plain = self.send(agent, "Use no tools. Write a 500-word description of a quiet forest.")

        def observe():
            status = self.status(agent)
            row = next(m for m in self.messages(agent) if m["id"] == plain)
            return (status, row) if status["turn_id"] or row["state"] == "completed" else None

        active, row = self.wait(observe, "no-tool turn active or already completed")
        if row["state"] == "completed":
            self.results["inconclusive"].append("No-tool turn ended before steering acceptance")
            return
        stale_marker = str(uuid.uuid4())
        try:
            guidance = self.steer(agent, active, f"Include {stale_marker} in your final answer.")
        except ControlError as error:
            if error.response["error"]["code"] != "no_active_turn":
                raise
            self.delivered(agent, plain)
            self.results["inconclusive"].append("No-tool turn ended during steering acceptance")
            return
        self.results["steering"]["no_tool"] = guidance
        self.delivered(agent, plain)
        self.assert_stale(agent, plain, guidance, stale_marker)
        for event in self.events(agent):
            require(
                not (
                    event["type"] == "tool_completed"
                    and event["data"]["turn_id"] == active["turn_id"]
                ),
                "No-tool steering used tools",
            )

    def held_steering(self, agent):
        held, turn, pids = self.gate(agent, "claude-held-interrupt")
        stale_marker = str(uuid.uuid4())
        guidance = self.steer(agent, turn, f"Include {stale_marker} in your final answer.")
        # Submitting proves writer dispatch, not codec receipt; only actual hook
        # receipts prove delivery. No successful hook is possible while gated.
        self.delivered(agent, guidance["message_id"], "submitting")
        self.results["steering"]["held_interrupt"] = dict(
            acceptance=guidance, observed="submitting"
        )
        self.save()
        self.call("interrupt", target=agent)
        self.delivered(agent, held, "interrupted")
        self.assert_stale(agent, held, guidance, stale_marker)
        self.gone(pids)

    def steering_receipts(self, agent, active, receipt_mode):
        receipts, seen = [], set()
        for event in self.events(agent):
            if (
                event["type"] == "steering_submitted"
                and event["data"]["turn_id"] == active["turn_id"]
            ):
                receipt = event["data"]["receipt"]
                require(
                    receipt["turnId"] == active["turn_id"], "Steering receipt targeted wrong turn"
                )
                require(
                    receipt["deliveryEvidence"]["kind"] == receipt_mode,
                    "Steering receipt evidence changed",
                )
                reply = receipt["deliveryEvidence"]["hookReply"]["response"]
                if reply["request_id"] not in seen:
                    seen.add(reply["request_id"])
                    receipts.append(reply["response"]["hookSpecificOutput"]["additionalContext"])
        return receipts

    def release(self):
        cleanup_all(
            *(
                lambda path=ready: atomic_write_text(path.with_suffix(".release"), "")
                for ready in self.workspace.glob("*.ready")
            )
        )

    def retain(self):
        super().retain()
        for agent in self.agents:
            stream = self.events(agent)
            atomic_write_text(
                self.output / (agent + "-messages.json"), json.dumps(self.messages(agent), indent=2)
            )
            assert_clean_executions(stream)
        self.gone(
            {
                name + "-" + key: value
                for name, identities in self.results["gates"].items()
                for key, value in identities.items()
            }
        )

    def cleanup(self):
        self.cleanup_until = time.monotonic() + 90
        with finalizer(self.save):
            super().cleanup()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    options(parser, cap=600)
    parser.add_argument("--provider", choices=("both", "codex", "claude"), default="codex")
    parser.add_argument("--codex-model", default=LIVE_CODEX_MODEL)
    parser.add_argument("--codex-changed-model", default="gpt-6-sol")
    parser.add_argument("--codex-changed-effort", default="medium")
    parser.add_argument("--claude-model", default="sonnet")
    parser.add_argument("--claude-changed-model", default="haiku")
    parser.add_argument("--claude-changed-effort", default="low")
    args = parser.parse_args()
    checked(
        parser,
        args,
        label="controls",
        agents=2 if args.provider == "both" else 1,
        models=dict(
            codex=args.codex_model,
            changed=args.codex_changed_model,
            effort=LIVE_CODEX_EFFORT,
            claude=args.claude_model,
        ),
        cap=600,
    )
    return Run(args).run()


if __name__ == "__main__":
    from release.progress import entry

    raise SystemExit(entry(main, name="scenario"))
