use super::*;
use support::wait::{POLL_INTERVAL, TEST_TIMEOUT, wait_until};

/// Wait until every accepted message has run: the agent is idle with nothing
/// queued, and its newest message or turn event is a finished turn. State alone
/// passes too early: a fresh spawn is idle while its body is queued, and a claimed
/// message has left the queue before its turn starts.
fn idle(f: &Harness, id: &str) {
    wait_until(
        TEST_TIMEOUT,
        POLL_INTERVAL,
        "idle agent after its last turn",
        || {
            let row = f.ok(json!({"op":"tree","root":id}))["agents"][0].clone();
            let idle = row["state"] == "idle" && row["queued"] == 0;
            let read = f.read(id);
            let last = read
                .events
                .iter()
                .rev()
                .find(|event| {
                    ["message_accepted", "turn_started", "turn_finished"]
                        .contains(&event.kind.as_str())
                })
                .map(|event| event.kind.clone());
            (idle && last.as_deref() == Some("turn_finished") && f.status(id).turn_id.is_none())
                .then_some(())
        },
    );
}

/// Poll `live` until a turn completes, returning every item and whether a
/// durable change was signalled.
fn until_turn_completed(
    f: &Harness,
    id: &str,
    cursor: Value,
) -> (Vec<harness::views::live::LiveItem>, bool) {
    let mut cursor = Some(cursor);
    let mut items = Vec::new();
    let mut durable = false;
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, "live turn completion", || {
        let page = f.live(json!({"op":"live","target":id,"after":cursor,"wait_ms":2000}));
        assert!(!page.resync, "{page:?}");
        cursor = page.cursor;
        durable |= page.durable_changed;
        items.extend(page.items);
        items
            .iter()
            .any(|item| item.kind == "turn_completed")
            .then_some(())
    });
    (items, durable)
}

fn text_of(items: &[harness::views::live::LiveItem], kind: &str) -> (String, Option<String>) {
    let matching: Vec<_> = items.iter().filter(|item| item.kind == kind).collect();
    assert!(!matching.is_empty(), "no {kind} in {items:?}");
    let item_id = matching[0].item_id.clone();
    assert!(
        matching.iter().all(|item| item.item_id == item_id),
        "{matching:?}"
    );
    (
        matching
            .iter()
            .map(|item| item.text.as_deref().unwrap())
            .collect(),
        item_id,
    )
}

/// The durable message text with this item key. The writer commits it after
/// the live item was published, so it is waited for.
fn durable_text(f: &Harness, id: &str, item_id: Option<&str>) -> String {
    let message = format!("durable message keyed {item_id:?}");
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, &message, || {
        f.read(id)
            .events
            .iter()
            .filter(|event| event.kind == "assistant_message")
            .find(|event| event.data["item_id"] == serde_json::to_value(item_id).unwrap())
            .map(|event| event.data["text"].as_str().unwrap().to_owned())
    })
}

#[test]
fn codex_live_items_stream_between_turn_markers_and_match_durable_messages() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({"live_deltas":true,"record_initialize":true}))
        .build();
    f.start();
    let id = f.spawn("reply: first");
    idle(&f, &id);
    let attached = f.attached(json!({"op":"attach","target":id}));
    assert_eq!(attached.agent.id, id);
    assert!(!attached.events.events.is_empty());
    assert_eq!((attached.queued, attached.mail), (0, 0));
    let answer = "streamed answer";
    f.ok(json!({"op":"send","target":id,"body":format!("reply: {answer}")}));
    let (items, durable) = until_turn_completed(&f, &id, attached.live.clone());
    let kinds: Vec<_> = items.iter().map(|item| item.kind.as_str()).collect();
    assert_eq!(kinds.first(), Some(&"turn_started"), "{kinds:?}");
    assert_eq!(kinds.last(), Some(&"turn_completed"), "{kinds:?}");
    let started = items.iter().find(|i| i.kind == "tool_started").unwrap();
    assert_eq!(
        (started.tool.as_deref(), started.text.as_deref()),
        (Some("command"), Some("echo live"))
    );
    let done = items.iter().find(|i| i.kind == "tool_completed").unwrap();
    assert_eq!(
        (done.status.as_deref(), done.text.as_deref()),
        (Some("completed"), Some("exit 0"))
    );
    assert_eq!(text_of(&items, "tool_output").0, "live\n");
    assert_eq!(text_of(&items, "reasoning").0, "Planning the answer.");
    let (text, item_id) = text_of(&items, "text");
    assert!(text.ends_with(answer), "{text}");
    // User-message echoes and raw reasoning never become live items.
    assert!(!items.iter().any(
        |i| serde_json::to_string(i).unwrap().contains("echoed input")
            || i.text.as_deref() == Some("raw")
    ));
    // The durable message carries the same key, so a viewer can replace streamed text.
    assert_eq!(durable_text(&f, &id, item_id.as_deref()), text);
    assert!(durable, "durable changes were not signalled");
    // Session processes opt in to displayed deltas only; discovery keeps all off.
    let all = json!([
        "item/agentMessage/delta",
        "item/reasoning/textDelta",
        "item/reasoning/summaryTextDelta",
        "item/commandExecution/outputDelta"
    ]);
    let raw = json!(["item/reasoning/textDelta"]);
    let opt_outs: Vec<Value> = fs::read_to_string(f.provider_home.join("initialize.jsonl"))
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["capabilities"][
            "optOutNotificationMethods"].clone()
        })
        .collect();
    assert!(opt_outs.contains(&raw), "{opt_outs:?}");
    assert!(
        opt_outs.iter().all(|value| *value == raw || *value == all),
        "{opt_outs:?}"
    );
    f.shutdown();
}

#[test]
fn claude_partial_text_streams_and_matches_its_native_message() {
    let mut f = claude::fixture();
    f.set_scenario(Provider::Claude, json!({"live_deltas":true}));
    f.start();
    let id = claude::spawn(&f, "reply: first");
    idle(&f, &id);
    let attached = f.attached(json!({"op":"attach","target":id}));
    let answer = "partial answer";
    f.ok(json!({"op":"send","target":id,"body":format!("reply: {answer}")}));
    let (items, _) = until_turn_completed(&f, &id, attached.live.clone());
    let (text, item_id) = text_of(&items, "text");
    assert!(
        text.ends_with(answer),
        "{text}\nATTACHED {attached:?}\nITEMS {items:?}\nEVENTS {:?}",
        f.read(&id)
    );
    assert_eq!(text_of(&items, "reasoning").0, "Planning.");
    assert!(item_id.as_deref().unwrap().starts_with("msg_"));
    assert_eq!(durable_text(&f, &id, item_id.as_deref()), text);
    f.shutdown();
}

#[test]
fn tree_long_polls_wake_on_spawn_and_restarts_resync_live_viewers() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let first = f.spawn("reply: one");
    idle(&f, &first);
    let tree = f.tree(json!({"op":"tree"}));
    let ids = |tree: &harness::views::TreeView| -> Vec<String> {
        tree.agents.iter().map(|a| a.id.clone()).collect()
    };
    assert_eq!(ids(&tree), std::slice::from_ref(&first));
    let row = &tree.agents[0];
    assert_eq!(
        (row.state.as_str(), row.queued, row.children),
        ("idle", 0, 0)
    );
    assert!(!tree.truncated);
    let second = f.spawn("reply: two");
    // The revision already moved, so this poll returns without waiting.
    let started = Instant::now();
    let next = f.tree(json!({"op":"tree","after":tree.version,"wait_ms":20000}));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(next.version["revision"].as_u64() > tree.version["revision"].as_u64());
    assert!(ids(&next).contains(&second));
    let subtree = f.tree(json!({"op":"tree","root":first}));
    assert_eq!(ids(&subtree), std::slice::from_ref(&first));
    // Every live agent, without waiting; `all` excludes `root`.
    let all = f.tree(json!({"op":"tree","all":true}));
    assert_eq!(ids(&all), [first.clone(), second.clone()]);
    let conflict = f.expect_err(
        json!({"op":"tree","all":true,"root":first}),
        "conflicting_arguments",
    );
    assert_eq!(conflict.error.field.as_deref().unwrap(), "/all");
    // Live without attach and invalid inputs are refused or resynced, not opened.
    let unknown_cursor = json!({"epoch":1,"generation":1,"seq":0,"durable":0});
    let page = f.live(json!({"op":"live","target":second,"after":unknown_cursor,"wait_ms":0}));
    assert_eq!((page.resync, &page.cursor), (true, &None));
    f.expect_err(
        json!({"op":"live","target":first,"after":unknown_cursor,"wait_ms":25001}),
        "invalid_value",
    );
    f.expect_err(json!({"op":"attach","target":"missing"}), "unknown_agent");
    // Batches accept only spawn, send and terminate, so viewers never reach the writer.
    let batch = f.call(json!({"op":"batch","operations":[{"op":"tree","request_id":"t"}]}));
    let batch = support::reply::Failure::parse(batch);
    assert_eq!(
        batch.error.field.as_deref().unwrap(),
        "/operations/0/op",
        "{batch:?}"
    );
    let attached = f.attached(json!({"op":"attach","target":first}));
    f.shutdown();
    f.start();
    let page = f.live(json!({"op":"live","target":first,"after":attached.live,"wait_ms":0}));
    assert_eq!((page.resync, &page.cursor), (true, &None));
    f.shutdown();
}

#[test]
fn waiting_viewers_never_hold_turns_and_shutdown_ends_their_polls() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({"live_deltas":true,"delta_delay":0.01}))
        .build();
    f.start();
    let id = f.spawn("reply: first");
    idle(&f, &id);
    // An attached viewer that never polls cannot slow its agent's turns.
    f.ok(json!({"op":"attach","target":id}));
    for body in ["reply: a", "reply: b", "reply: c"] {
        f.ok(json!({"op":"send","target":id,"body":body}));
    }
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, "three viewed turns", || {
        let read = f.read(&id);
        (read
            .events
            .iter()
            .filter(|e| e.kind == "assistant_message")
            .count()
            >= 4)
            .then_some(())
    });
    let attached = f.attached(json!({"op":"attach","target":id}));
    let root = f.root.clone();
    let target = id.clone();
    // Follow like a viewer: shutdown's own events may answer one poll first, but
    // no poll waits out its 25 s and the loop ends with a stopping daemon.
    let poll = std::thread::spawn(move || {
        let started = Instant::now();
        let mut after = attached.live.clone();
        loop {
            let polled = Instant::now();
            let reply = runtime::client(
                &root,
                &Uuid::new_v4().to_string(),
                json!({"op":"live","target":target,"after":after,"wait_ms":25000}),
            );
            let code = match &reply {
                Ok(value) => value["error"]["code"].as_str().map(str::to_owned),
                Err(error) => Some(error.code().to_owned()),
            };
            assert!(polled.elapsed() < Duration::from_secs(15), "{reply:?}");
            if let Some(code) = code {
                return (code, started.elapsed());
            }
            after = reply.unwrap()["cursor"].clone();
        }
    });
    std::thread::sleep(Duration::from_millis(300));
    let stopping = Instant::now();
    f.shutdown();
    assert!(stopping.elapsed() < Duration::from_secs(10));
    let (code, waited) = poll.join().unwrap();
    assert!(
        code == "daemon_stopping" || code == "daemon_unavailable",
        "{code}"
    );
    assert!(waited < Duration::from_secs(15), "{waited:?}");
}

/// Start a long poll that must be answered by a change, not its timeout.
fn background_poll(f: &Harness, request: Value) -> std::thread::JoinHandle<Value> {
    let root = f.root.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let reply = runtime::client(&root, &Uuid::new_v4().to_string(), request).unwrap();
        assert!(started.elapsed() < Duration::from_secs(10), "{reply}");
        reply
    })
}

/// A tree version and live cursor taken once the agent's history is quiet.
fn quiet_versions(f: &Harness, id: &str) -> (Value, Value) {
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, "quiet viewer cursors", || {
        let version = f.ok(json!({"op":"tree"}))["version"].clone();
        let live = f.attached(json!({"op":"attach","target":id})).live.clone();
        let tree = f.tree(json!({"op":"tree","after":version,"wait_ms":400}));
        let page = f.live(json!({"op":"live","target":id,"after":live,"wait_ms":400}));
        (tree.version == version && page.cursor.as_ref() == Some(&live)).then_some((version, live))
    })
}

#[test]
fn approvals_and_termination_wake_waiting_tree_and_live_polls() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let id = f.spawn("reply: ready");
    idle(&f, &id);
    let ask = json!({"request_id":"r","message":"Deploy now?"});
    f.ok(json!({"op":"send","target":id,"body":format!("ask:{ask}")}));
    let approval = f
        .wait(&id, |a| a.waiting_on.as_deref() == Some("human"))
        .approval_id
        .clone();
    let rows = |tree: &Value| tree["agents"].as_array().unwrap().clone();
    let row = |tree: &Value| rows(tree).into_iter().find(|row| row["id"] == id);
    assert_eq!(
        row(&f.ok(json!({"op":"tree"}))).unwrap()["approval_id"],
        serde_json::to_value(&approval).unwrap()
    );
    // Closing the approval wakes both viewers, and the row no longer shows it.
    let (version, live) = quiet_versions(&f, &id);
    let tree = background_poll(&f, json!({"op":"tree","after":version,"wait_ms":25000}));
    let page = background_poll(
        &f,
        json!({"op":"live","target":id,"after":live,"wait_ms":25000}),
    );
    std::thread::sleep(Duration::from_millis(300));
    f.ok(json!({"op":"respond","approval_id":approval,"action":"cancel"}));
    assert_ne!(tree.join().unwrap()["version"], version);
    assert_eq!(page.join().unwrap()["durable_changed"], true);
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, "closed approval row", || {
        row(&f.ok(json!({"op":"tree"})))
            .is_some_and(|row| row["approval_id"].is_null())
            .then_some(())
    });
    // Termination removes the root from the tree and wakes its viewer.
    idle(&f, &id);
    let (version, live) = quiet_versions(&f, &id);
    let tree = background_poll(&f, json!({"op":"tree","after":version,"wait_ms":25000}));
    let page = background_poll(
        &f,
        json!({"op":"live","target":id,"after":live,"wait_ms":25000}),
    );
    std::thread::sleep(Duration::from_millis(300));
    f.ok(json!({"op":"terminate","target":id}));
    assert_ne!(tree.join().unwrap()["version"], version);
    assert_eq!(page.join().unwrap()["durable_changed"], true);
    wait_until(
        TEST_TIMEOUT,
        POLL_INTERVAL,
        "ended root leaves the tree",
        || row(&f.ok(json!({"op":"tree"}))).is_none().then_some(()),
    );
    f.shutdown();
}

#[test]
fn attach_pages_forward_and_backward_over_durable_history() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let id = f.spawn("reply: one");
    idle(&f, &id);
    f.ok(json!({"op":"send","target":id,"body":"reply: two"}));
    idle(&f, &id);
    let all = f.read(&id).events;
    assert!(all.len() > 4, "{all:?}");
    let newest = f.attached(json!({"op":"attach","target":id,"limit":2}));
    assert_eq!(
        serde_json::to_value(&newest.events.events).unwrap(),
        serde_json::to_value(&all[all.len() - 2..]).unwrap()
    );
    // Page backwards from the newest page until no older history remains.
    let mut collected = newest.events.events;
    let mut before = collected[0].seq;
    loop {
        let page = f.attached_history(json!({"op":"attach","target":id,"before":before,"limit":3}));
        let events = page.events.events;
        assert!(events.len() <= 3);
        assert!(!page.events.history_expired);
        collected.splice(0..0, events);
        match page.events.older {
            Some(older) => before = older,
            None => break,
        }
    }
    assert_eq!(
        serde_json::to_value(&collected).unwrap(),
        serde_json::to_value(&all).unwrap()
    );
    // The forward page after a durable cursor matches `read`.
    let after = f.attached(json!({"op":"attach","target":id,"after":all[1].seq}));
    assert_eq!(
        serde_json::to_value(&after.events.events).unwrap(),
        serde_json::to_value(&all[2..]).unwrap()
    );
    assert_eq!(after.events.cursor, all[all.len() - 1].seq);
    for (args, code, field) in [
        (
            json!({"after":1,"before":5}),
            "conflicting_arguments",
            "/before",
        ),
        (json!({"before":0}), "out_of_range", "/before"),
        (json!({"after":"x"}), "invalid_value", "/after"),
        (json!({"limit":0}), "out_of_range", "/limit"),
    ] {
        let mut request = json!({"op":"attach","target":id});
        request
            .as_object_mut()
            .unwrap()
            .extend(args.as_object().unwrap().clone());
        let error = f.expect_err(request, code);
        assert_eq!(error.error.field.as_deref().unwrap(), field, "{error:?}");
    }
    f.shutdown();
}
