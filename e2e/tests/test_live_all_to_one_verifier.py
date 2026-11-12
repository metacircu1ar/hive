"""Keep the corruption cases for exact message and turn provenance."""

import copy
import unittest

from e2e.scripts.run_ping_pong_all_to_one import verify


def fixture(children=10):
    events, agents, messages = {}, [], []
    sequence = 0

    def event(agent, kind, **data):
        nonlocal sequence
        sequence += 1
        events[agent].append(dict(seq=sequence, execution_id="exec-" + agent, type=kind, data=data))

    def message(sender, recipient, body):
        value = dict(
            id=f"m{len(messages)}", sender=sender, recipient=recipient, body=body, state="completed"
        )
        messages.append(value)
        return value

    def turn(agent, msg):
        ident = "turn-" + msg["id"]
        event(agent, "turn_started", message_id=msg["id"], turn_id=ident)
        return ident

    def agent(ident, parent, initial):
        role = "pong" if parent is None else "ping"
        model = "gpt-5.6-sol" if parent is None else "gpt-5.6-terra"
        events[ident] = []
        agents.append(
            dict(
                id=ident,
                parent_id=parent,
                role=role,
                model=model,
                effort="high",
                lifetime="independent",
                state="stopped",
                task_outcome="completed" if parent is None else "cancelled",
                thread_id="thread-" + ident,
                turn_id=None,
                control=None,
                error=None,
                expired=False,
            )
        )
        event(ident, "spawned", parent_id=parent, initial_message_id=initial["id"])
        event(ident, "execution_starting")
        event(ident, "session_ready", model=model, effort="high", thread_id="thread-" + ident)

    initial = message("human", "root", "task")
    agent("root", None, initial)
    turn("root", initial)
    for i in range(children):
        child = f"child-{i}"
        startup = message("root", child, "start")
        agent(child, "root", startup)
        startup_turn = turn(child, startup)
        ping = message(child, "root", "ping")
        event(
            "root",
            "message_accepted",
            id=ping["id"],
            sender=child,
            sender_turn_id=startup_turn,
            body="ping",
        )
        ping_turn = turn("root", ping)
        reply = message("root", child, "pong")
        event(
            child,
            "message_accepted",
            id=reply["id"],
            sender="root",
            sender_turn_id=ping_turn,
            body="pong",
        )
        reply_turn = turn(child, reply)
        receipt = message(child, "root", "received pong")
        event(
            "root",
            "message_accepted",
            id=receipt["id"],
            sender=child,
            sender_turn_id=reply_turn,
            body="received pong",
        )
        turn("root", receipt)
        event(child, "turn_finished", message_id=reply["id"], status="completed")
    for i in range(children):
        child = f"child-{i}"
        event(child, "termination_requested", by="root")
        event(child, "provider_shutdown", confirmed=True, quiescent=True)
        event(child, "execution_stopped")
    event("root", "completion_requested", by="root")
    event("root", "task_completed")
    event("root", "provider_shutdown", confirmed=True, quiescent=True)
    event("root", "execution_stopped")
    return dict(
        root_id="root",
        expected_children=children,
        agents=agents,
        messages=messages,
        events=events,
        expected_roles={
            "pong": dict(model="gpt-5.6-sol", effort="high"),
            "ping": dict(model="gpt-5.6-terra", effort="high"),
        },
    )


class LiveVerifier(unittest.TestCase):
    def test_pooled_conversation_release_without_process_shutdown(self):
        value = fixture(2)
        for events in value["events"].values():
            for event in events:
                if event["type"] == "provider_shutdown":
                    event["type"] = "provider_session_released"
                    event["data"].update(method="session_released", reaped=False, exit_code=None)
        self.assertEqual(verify(value)["confirmed_agent_shutdowns"], 3)
        for field in ("confirmed", "quiescent"):
            broken = copy.deepcopy(value)
            release = next(
                e for e in broken["events"]["root"] if e["type"] == "provider_session_released"
            )
            release["data"][field] = False
            with self.assertRaises(RuntimeError):
                verify(broken)
        for kind in ("provider_session_released", "execution_stopped"):
            broken = copy.deepcopy(value)
            broken["events"]["root"] = [e for e in broken["events"]["root"] if e["type"] != kind]
            with self.assertRaises(RuntimeError):
                verify(broken)

    def test_small_default_keeps_the_same_provenance_checks(self):
        result = verify(fixture(2))
        self.assertEqual(result["confirmed_agent_shutdowns"], 3)
        self.assertEqual((result["pings"], result["pongs"], result["receipts"]), (2, 2, 2))

    def test_original_positive_and_seven_corruptions(self):
        original = fixture()
        self.assertEqual(verify(original)["confirmed_agent_shutdowns"], 11)
        mutations = [
            lambda e: e["agents"].pop(),
            lambda e: e["agents"][1].update(task_outcome="completed"),
            lambda e: next(m for m in e["messages"] if m["body"] == "pong").update(
                state="accepted"
            ),
            lambda e: next(
                x
                for x in e["events"]["root"]
                if x["type"] == "message_accepted" and x["data"]["body"] == "received pong"
            )["data"].update(sender_turn_id="wrong-turn"),
            lambda e: next(
                x for x in e["events"]["child-0"] if x["type"] == "termination_requested"
            )["data"].update(by="human"),
            lambda e: next(x for x in e["events"]["child-0"] if x["type"] == "provider_shutdown")[
                "data"
            ].update(confirmed=False),
            lambda e: next(
                x for x in e["events"]["child-0"] if x["type"] == "termination_requested"
            ).update(seq=0),
        ]
        for index, mutate in enumerate(mutations):
            with self.subTest(index=index):
                broken = copy.deepcopy(original)
                mutate(broken)
                with self.assertRaises(RuntimeError):
                    verify(broken)
