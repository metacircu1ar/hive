//! Bounded native product coverage, independent of the experimental Python harness.
use super::*;
use harness::views::{EventView, MessageView, listing::ListedAgentView};
use std::collections::{BTreeMap, HashSet};

struct Evidence {
    root_id: String,
    agents: Vec<ListedAgentView>,
    events: BTreeMap<String, Vec<EventView>>,
    messages: Vec<MessageView>,
}

#[track_caller]
fn one<'a, T: std::fmt::Debug>(mut values: impl Iterator<Item = &'a T>) -> &'a T {
    let first = values.next().expect("Missing evidence record");
    assert!(
        values.next().is_none(),
        "Duplicate evidence record: {first:?}"
    );
    first
}
fn events<'a>(e: &'a Evidence, agent: &str, kind: &str) -> Vec<&'a EventView> {
    e.events
        .get(agent)
        .unwrap()
        .iter()
        .filter(|v| v.kind == kind)
        .collect()
}
fn event<'a>(e: &'a Evidence, agent: &str, kind: &str) -> &'a EventView {
    one(events(e, agent, kind).into_iter())
}
fn seq(v: &EventView) -> u64 {
    v.seq
}

fn evidence(f: &Harness, root: &str) -> Evidence {
    let all = f.listed(json!({"op":"agents","limit":200}));
    let agents: Vec<_> = all
        .agents
        .into_iter()
        .filter(|a| a.id == root || a.parent_id.as_deref() == Some(root))
        .collect();
    let mut records = BTreeMap::new();
    for agent in &agents {
        records.insert(agent.id.clone(), f.read(&agent.id).events);
    }
    Evidence {
        root_id: root.into(),
        agents,
        events: records,
        messages: f.mailbox(root).messages,
    }
}

fn check_agent(e: &Evidence, a: &ListedAgentView, shared: bool) {
    let id = a.id.as_str();
    assert_eq!(a.state, "stopped");
    assert!(a.turn_id.is_none() && a.control.is_none());
    assert!(!a.expired);
    assert!(a.error.is_none());
    let expected = if a.role.as_deref() == Some("pong") {
        "gpt-5.6-sol"
    } else {
        "gpt-5.6-terra"
    };
    assert_eq!(a.model, expected);
    assert_eq!(a.effort, "high");
    let ready = events(e, id, "session_ready");
    assert!(!ready.is_empty());
    for item in ready {
        assert_eq!(item.data["model"], expected);
        assert_eq!(item.data["effort"], a.effort);
        assert_eq!(
            item.data["thread_id"],
            serde_json::to_value(&a.thread_id).unwrap()
        );
    }
    for kind in ["execution_failed", "model_rerouted"] {
        assert!(events(e, id, kind).is_empty());
    }
    let starts = events(e, id, "execution_starting");
    let shutdowns = events(e, id, "provider_shutdown");
    assert!(!starts.is_empty());
    if shared {
        check_releases(e.events.get(id).unwrap(), &starts);
        return;
    }
    assert_eq!(starts.len(), shutdowns.len());
    for start in starts {
        let shutdown = one(shutdowns
            .iter()
            .copied()
            .filter(|s| s.execution_id == start.execution_id));
        assert_eq!(shutdown.data["confirmed"], true);
        assert!(seq(start) < seq(shutdown));
    }
    event(e, id, "spawned");
}

pub(super) fn check_releases(records: &[EventView], starts: &[&EventView]) {
    let releases: Vec<_> = records
        .iter()
        .filter(|e| e.kind == "provider_session_released")
        .collect();
    let shutdowns: Vec<_> = records
        .iter()
        .filter(|e| e.kind == "provider_shutdown")
        .collect();
    for start in starts {
        let stopped = one(records
            .iter()
            .filter(|e| e.kind == "execution_stopped")
            .filter(|s| s.execution_id == start.execution_id));
        let proof: Vec<_> = releases
            .iter()
            .chain(&shutdowns)
            .copied()
            .filter(|s| {
                s.execution_id == start.execution_id && seq(start) < seq(s) && seq(s) < seq(stopped)
            })
            .collect();
        assert!(!proof.is_empty(), "Missing release for {start:?}");
        assert!(
            proof
                .iter()
                .all(|p| p.data["confirmed"] == true && p.data["quiescent"] == true)
        );
    }
    one(records.iter().filter(|e| e.kind == "spawned"));
}

fn check_exchange(e: &Evidence, child: &str, startup: &MessageView, cancelled: &EventView) -> u64 {
    let root = e.root_id.as_str();
    let messages = &e.messages;
    let find = |sender: &str, recipient: &str, body: &str| {
        one(messages
            .iter()
            .filter(|m| m.sender == sender && m.recipient == recipient && m.body == body))
    };
    let ping = find(child, root, "ping");
    let reply = find(root, child, "pong");
    let ack = find(child, root, "received pong");
    assert_eq!(ping.state, "completed");
    assert_eq!(ack.state, "completed");
    assert!(reply.state == "completed" || reply.state == "interrupted");
    for (sent, trigger, sender, recipient) in [
        (ping, startup, child, root),
        (reply, ping, root, child),
        (ack, reply, child, root),
    ] {
        let turn = one(events(e, sender, "turn_started")
            .into_iter()
            .filter(|t| t.data["message_id"] == trigger.id));
        let accepted = one(events(e, recipient, "message_accepted")
            .into_iter()
            .filter(|a| a.data["id"] == sent.id));
        assert_eq!(accepted.data["sender"], sender);
        assert_eq!(accepted.data["sender_turn_id"], turn.data["turn_id"]);
        assert!(seq(turn) < seq(accepted));
        if sent.id == ack.id {
            assert!(seq(accepted) < seq(cancelled));
        }
    }
    let finished = one(events(e, child, "turn_finished")
        .into_iter()
        .filter(|t| t.data["message_id"] == reply.id));
    assert_eq!(finished.data["status"], reply.state);
    if reply.state == "interrupted" {
        assert!(seq(cancelled) < seq(finished));
    }
    seq(one(events(e, root, "turn_started")
        .into_iter()
        .filter(|t| t.data["message_id"] == ack.id)))
}

fn check(e: &Evidence, children: usize, shared: bool) {
    let root = e.root_id.as_str();
    let agents = &e.agents;
    assert_eq!(agents.len(), children + 1);
    let parent = one(agents.iter().filter(|a| a.id == root));
    assert!(parent.parent_id.is_none());
    assert_eq!(parent.role.as_deref(), Some("pong"));
    assert_eq!(parent.task_outcome, "completed");
    let threads: HashSet<_> = agents
        .iter()
        .map(|a| a.thread_id.as_deref().unwrap())
        .collect();
    assert_eq!(threads.len(), agents.len());
    let requested = event(e, root, "completion_requested");
    assert_eq!(requested.data["by"], root);
    assert!(seq(requested) < seq(event(e, root, "task_completed")));
    let mut latest_receipt = 0;
    let mut earliest_cancel = u64::MAX;
    let mut expected = HashSet::new();
    let messages = &e.messages;
    let initial = one(messages
        .iter()
        .filter(|m| m.sender == "human" && m.recipient == root));
    assert_eq!(initial.state, "completed");
    expected.insert(initial.id.as_str());
    for a in agents {
        check_agent(e, a, shared);
        let id = a.id.as_str();
        if id == root {
            continue;
        }
        assert_eq!(a.parent_id.as_deref(), Some(root));
        assert_eq!(a.role.as_deref(), Some("ping"));
        assert_eq!(a.lifetime, hive::config::Lifetime::Independent);
        assert_eq!(a.task_outcome, "cancelled");
        for kind in ["completion_requested", "task_completed", "lifetime_expired"] {
            assert!(events(e, id, kind).is_empty());
        }
        let cancelled = event(e, id, "termination_requested");
        assert_eq!(cancelled.data["by"], root);
        assert!(seq(cancelled) < seq(requested));
        earliest_cancel = earliest_cancel.min(seq(cancelled));
        let spawned = event(e, id, "spawned");
        assert_eq!(spawned.data["parent_id"], root);
        let startup = one(messages
            .iter()
            .filter(|m| spawned.data["initial_message_id"] == m.id));
        assert_eq!(startup.sender, root);
        assert_eq!(startup.recipient, id);
        assert_eq!(startup.state, "completed");
        latest_receipt = latest_receipt.max(check_exchange(e, id, startup, cancelled));
        expected.insert(startup.id.as_str());
        for (sender, recipient, body) in [
            (id, root, "ping"),
            (root, id, "pong"),
            (id, root, "received pong"),
        ] {
            expected.insert(
                one(messages
                    .iter()
                    .filter(|m| m.sender == sender && m.recipient == recipient && m.body == body))
                .id
                .as_str(),
            );
        }
    }
    assert!(latest_receipt < earliest_cancel);
    assert_eq!(messages.len(), expected.len());
    assert!(messages.iter().all(|m| expected.contains(m.id.as_str())));
}

fn root(f: &Harness) -> String {
    f.call(json!({"op":"spawn","role":"pong","cwd":f.tmp.path(),
        "body":"ping pong scenario"}))["agent_id"]
        .as_str()
        .unwrap()
        .into()
}
fn wait(f: &Harness, root: &str) {
    f.wait_timeout(root, Duration::from_secs(90), |a| {
        a.task_outcome == "completed" && a.state == "stopped"
    });
}

#[test]
fn ten_ping_agents_exchange_messages_and_are_explicitly_terminated_by_pong() {
    let mut f = Harness::new();
    f.start();
    let id = root(&f);
    wait(&f, &id);
    let all = f.listed(json!({"op":"agents","limit":200}));
    let tail = f.listed(json!({"op":"agents","after":all.next_after,"limit":200}));
    assert!(tail.agents.is_empty());
    assert!(tail.next_after.is_none());
    assert_eq!(all.agents.len(), 11);
    for a in &all.agents {
        f.wait(a.id.as_str(), |a| a.state == "stopped");
    }
    let e = evidence(&f, &id);
    check(&e, 10, true);
    f.shutdown();
}

#[test]
fn fifty_pairs_keep_messages_isolated_and_settle_beyond_resident_capacity() {
    let mut f = Harness::builder(Provider::Codex)
        .runtime(
            json!({"max_loaded":8,"max_providers":2,"sessions_per_codex_process":4,
            "max_starting":4,"max_active_turns":4,"idle_seconds":0,"cohort_min_seconds":1}),
        )
        .scenario(json!({"ping_count":1}))
        .build();
    f.start();
    let roots: Vec<_> = (0..50).map(|_| root(&f)).collect();
    for id in &roots {
        wait(&f, id);
    }
    let all = f.listed(json!({"op":"agents","limit":200}));
    let agents = &all.agents;
    let tail = f.listed(json!({"op":"agents","after":all.next_after,"limit":200}));
    assert!(tail.agents.is_empty());
    assert!(tail.next_after.is_none());
    assert_eq!(agents.len(), 100);
    let threads: HashSet<_> = agents
        .iter()
        .map(|a| a.thread_id.as_deref().unwrap())
        .collect();
    assert_eq!(threads.len(), 100);
    for a in agents {
        f.wait(a.id.as_str(), |a| a.state == "stopped");
    }
    for id in &roots {
        check(&evidence(&f, id), 1, true);
    }
    f.shutdown();
    let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
    let count = |sql| db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(count("SELECT active FROM runtime_counts"), 0);
    assert_eq!(count("SELECT count(*) FROM messages"), 250);
    assert_eq!(
        count("SELECT count(*) FROM messages WHERE state NOT IN ('completed','interrupted')"),
        0
    );
    assert!(count("SELECT count(*) FROM events WHERE kind='provider_started'") >= 100);
    assert_eq!(
        count(
            "SELECT count(*) FROM events e WHERE e.kind='provider_started' AND NOT EXISTS \
                (SELECT 1 FROM provider_events p WHERE \
                p.provider=json_extract(e.data,'$.provider_id') AND \
                p.kind='started')"
        ),
        0
    );
    assert_eq!(
        count(
            "SELECT count(*) FROM (SELECT provider FROM provider_events GROUP BY provider \
                HAVING sum(kind='started') != 1 OR sum(kind='shutdown') != 1 OR min(CASE WHEN \
                kind='started' THEN seq END) >= min(CASE WHEN kind='shutdown' THEN seq \
                END))"
        ),
        0
    );
    assert_eq!(
        count(
            "SELECT count(*) FROM provider_events WHERE kind='shutdown' AND \
                json_extract(data,'$.shutdown.confirmed') IS NOT \
                1"
        ),
        0
    );
}
