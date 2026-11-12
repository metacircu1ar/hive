"""Verify real mixed-provider pairs using message and turn provenance."""

from e2e.scripts.harness.checks import one, require
from e2e.scripts.live_pair_memory import distribution


def verify(evidence):
    agents, events, messages = (evidence[k] for k in ("agents", "events", "messages"))
    roots = evidence["roots"]  # agent ID -> unique nonce
    require(len(agents) == 2 * len(roots), "Wrong agent count")
    require(len({a["thread_id"] for a in agents}) == len(agents), "Shared conversation")
    require({a["id"] for a in agents if a["parent_id"] is None} == set(roots), "Unexpected roots")
    used, latencies = set(), []

    def of(aid, kind):
        return [e for e in events[aid] if e["type"] == kind]

    def turn(aid, mid):
        return one([e for e in of(aid, "turn_started") if e["data"]["message_id"] == mid], "turn")

    def message(sender, recipient, body):
        m = one(
            [
                m
                for m in messages
                if m["sender"] == sender and m["recipient"] == recipient and m["body"] == body
            ],
            "message",
        )
        used.add(m["id"])
        return m

    def sent(m, trigger):
        origin = turn(m["sender"], trigger["id"])
        accepted = one(
            [e for e in of(m["recipient"], "message_accepted") if e["data"]["id"] == m["id"]],
            "acceptance",
        )
        require(
            accepted["data"]["sender_turn_id"] == origin["data"]["turn_id"]
            and origin["seq"] < accepted["seq"],
            "Wrong sending turn",
        )
        return accepted

    for a in agents:
        aid = a["id"]
        expected = evidence["expected_roles"][a["role"]]
        require(
            a["state"] == "stopped"
            and a["turn_id"] is None
            and a["control"] is None
            and a["error"] is None
            and not a["expired"],
            "Agent failed or active",
        )
        require(a["lifetime"] == "independent", "Wrong lifetime")
        ready = one(of(aid, "session_ready"), "session")
        require(
            a["model"] == ready["data"]["model"] == expected["model"]
            and a["effort"] == ready["data"]["effort"] == expected["effort"]
            and a["thread_id"] == ready["data"]["thread_id"],
            "Wrong model/session",
        )
        start = one(of(aid, "execution_starting"), "execution")
        stop = one(of(aid, "provider_session_released") or of(aid, "provider_shutdown"), "release")
        stopped = one(of(aid, "execution_stopped"), "execution stop")
        require(
            start["execution_id"] == stop["execution_id"] == stopped["execution_id"]
            and stop["data"]["confirmed"]
            and stop["data"]["quiescent"]
            and start["seq"] < stop["seq"] < stopped["seq"],
            "Unconfirmed release",
        )
        require(
            not any(
                of(aid, kind)
                for kind in (
                    "execution_failed",
                    "model_rerouted",
                    "control_requested",
                    "lifetime_expired",
                )
            ),
            "Failure, reroute, intervention or expiry",
        )

    for root, nonce in roots.items():
        parent = one([a for a in agents if a["id"] == root], "parent")
        child = one([a for a in agents if a["parent_id"] == root], "child")
        cid = child["id"]
        for sender, request, target, op in (
            (root, "ping-", "@child", "send"),
            (cid, "pong-", "@parent", "send"),
            (root, "stop-", "@child", "terminate"),
        ):
            accepted = one(
                [
                    r
                    for r in evidence["requests"]
                    if r["sender"] == sender and r["id"] == request + nonce
                ],
                "relative request",
            )
            require(
                accepted["body"]["target"] == target and accepted["body"]["op"] == op,
                "Agent did not use the required relative target",
            )
        require(
            parent["role"] == "live-ping"
            and parent["task_outcome"] == "completed"
            and child["role"] == "live-pong"
            and child["task_outcome"] == "cancelled",
            "Wrong pair outcomes",
        )
        body = evidence["initial_bodies"][nonce]
        initial = message("human", root, body)
        startup = message(root, cid, body)
        start = message("human", root, "START " + nonce)
        ping = message(root, cid, "PING " + nonce)
        pong = message(cid, root, "PONG " + nonce)
        require(
            all(m["state"] == "completed" for m in (initial, startup, start, pong)),
            "Unfinished parent/startup turns",
        )
        require(ping["state"] in ("completed", "interrupted"), "Undelivered ping")
        for aid, m in ((root, initial), (cid, startup)):
            spawn = one(of(aid, "spawned"), "spawn")
            require(spawn["data"]["initial_message_id"] == m["id"], "Wrong initial input")
        # Spawn records don't have message_accepted, so use its source-turn evidence.
        child_spawn = one(of(cid, "spawned"), "child spawn")
        require(
            child_spawn["data"]["parent_id"] == root
            and turn(root, initial["id"])["seq"] < child_spawn["seq"],
            "Wrong spawning parent",
        )
        ping_event, pong_event = sent(ping, start), sent(pong, ping)
        receipt = turn(root, pong["id"])
        cancel = one(of(cid, "termination_requested"), "child cancellation")
        complete = one(of(root, "completion_requested"), "parent completion")
        require(
            cancel["data"]["by"] == root
            and complete["data"]["by"] == root
            and receipt["seq"] < cancel["seq"] < complete["seq"],
            "Wrong cleanup causality",
        )
        require(
            not of(root, "termination_requested") and not of(cid, "completion_requested"),
            "Wrong termination mechanism",
        )
        end = one(
            [e for e in of(cid, "turn_finished") if e["data"]["message_id"] == ping["id"]],
            "ping turn end",
        )
        require(end["data"]["status"] == ping["state"], "Inconsistent ping outcome")
        if ping["state"] == "interrupted":
            require(cancel["seq"] < end["seq"], "Ping interrupted before cancellation")
        delta = pong_event["timestamp"] - ping_event["timestamp"]
        require(delta >= 0, "Invalid message timing")
        latencies.append(delta)
    require(
        len(messages) == len(used) and used == {m["id"] for m in messages},
        "Extra or duplicate messages",
    )
    return dict(
        result="passed",
        agents=len(agents),
        pairs=len(roots),
        messages=len(messages),
        confirmed_conversation_releases=len(agents),
        relative_target_requests=3 * len(roots),
        ping_to_pong_seconds=distribution(latencies),
        latency_precision="Each event delta has less than one second error in either direction.",
    )
