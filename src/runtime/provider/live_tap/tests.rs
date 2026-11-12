use super::*;
use serde_json::json;

#[test]
fn queued_delta_memory_is_bounded_and_overflow_always_resyncs() {
    let delta = |thread: String, bytes: usize| Delta {
        thread,
        event: Event {
            text: Some("x".repeat(bytes)),
            ..Event::new(Kind::Text)
        },
    };
    let mut queue = Deltas::default();
    queue.push(1, delta("old".into(), QUEUED_BYTES / 2));
    queue.push(2, delta("middle".into(), QUEUED_BYTES / 2));
    queue.push(3, delta("new".into(), QUEUED_BYTES / 2));
    assert!(queue.bytes <= QUEUED_BYTES);
    let first = queue.before(Some(3));
    assert_eq!(first.deltas.len(), 0);
    assert!(first.gaps.contains(&"old".into()) && first.gaps.contains(&"middle".into()));
    assert_eq!(queue.before(None).deltas[0].thread, "new");
    assert_eq!(queue.bytes, 0);
    queue.push(4, delta("oversized".into(), QUEUED_BYTES));
    let dropped = queue.before(None);
    assert!(dropped.deltas.is_empty());
    assert_eq!(dropped.gaps, ["oversized"]);
    queue.push(5, delta("x".repeat(GAP_BYTES + 1), QUEUED_BYTES));
    let global = queue.before(None);
    assert!(global.all_gaps && global.gaps.is_empty());
    assert!(queue.is_empty() && queue.gap_bytes == 0 && queue.bytes == 0);
    // Spare String allocation also counts, even when almost none is filled.
    let mut reserved = String::with_capacity(QUEUED_BYTES + 1);
    reserved.push('x');
    queue.push(
        6,
        Delta {
            thread: "reserved".into(),
            event: Event {
                text: Some(reserved),
                ..Event::default()
            },
        },
    );
    assert!(queue.before(None).deltas.is_empty());
}

fn frame(value: Value) -> IncomingFrame {
    IncomingFrame::parse(value)
}

#[test]
fn only_displayed_delta_families_are_intercepted() {
    for (method, kind) in [
        (AGENT_MESSAGE_DELTA, Some(DeltaKind::Shown(Kind::Text))),
        (
            COMMAND_OUTPUT_DELTA,
            Some(DeltaKind::Shown(Kind::ToolOutput)),
        ),
        (
            REASONING_SUMMARY_DELTA,
            Some(DeltaKind::Shown(Kind::Reasoning)),
        ),
        (RAW_REASONING_DELTA, Some(DeltaKind::Hidden)),
        ("item/completed", None),
        ("turn/started", None),
    ] {
        assert_eq!(delta_kind(&json!({"method": method, "params": {}})), kind);
    }
    assert_eq!(delta_kind(&json!({"id": 1, "result": {}})), None);
    let raw = json!({"method": AGENT_MESSAGE_DELTA, "params":
        {"threadId":"t","turnId":"u","itemId":"i","delta":"Hel"}});
    assert_eq!(
        delta(Kind::Text, &raw),
        Some(Delta {
            thread: "t".into(),
            event: Event {
                turn_id: Some("u".into()),
                item_id: Some("i".into()),
                text: Some("Hel".into()),
                ..Event::new(Kind::Text)
            },
        })
    );
    // Malformed deltas are ignored rather than failing the provider.
    for params in [
        json!({}),
        json!({"threadId": 1, "delta": "x"}),
        json!({"threadId": "t"}),
    ] {
        assert_eq!(
            delta(
                Kind::Text,
                &json!({"method": AGENT_MESSAGE_DELTA, "params": params})
            ),
            None
        );
    }
}

#[test]
fn queued_deltas_split_by_read_order_and_overflow_records_gaps() {
    let text = |thread: &str| Delta {
        thread: thread.into(),
        event: Event::new(Kind::Text),
    };
    let mut deltas = Deltas::default();
    for (seq, thread) in [(1, "a"), (2, "b"), (4, "c")] {
        deltas.push(seq, text(thread));
    }
    // Only deltas read before the next frame are ready; the rest wait for it.
    let ready = deltas.before(Some(3));
    let threads: Vec<_> = ready.deltas.iter().map(|d| d.thread.as_str()).collect();
    assert_eq!(threads, ["a", "b"]);
    assert!(ready.gaps.is_empty() && !ready.all_gaps);
    assert_eq!(deltas.before(None).deltas.len(), 1);
    for index in 0..(QUEUED_DELTAS + 3) {
        deltas.push(index as u64, text(&index.to_string()));
    }
    let ready = deltas.before(None);
    assert_eq!(ready.deltas.len(), QUEUED_DELTAS);
    assert_eq!(ready.deltas[0].thread, "3");
    let mut gaps = ready.gaps;
    gaps.sort();
    assert_eq!(gaps, ["0", "1", "2"]);
    assert!(!ready.all_gaps);
    // Past the remembered set, every thread resyncs.
    for index in 0..=(QUEUED_DELTAS + QUEUED_GAPS) {
        deltas.push(index as u64, text(&index.to_string()));
    }
    let ready = deltas.before(None);
    assert!(ready.gaps.is_empty()); // Global resync supersedes individual identities.
    assert!(ready.all_gaps);
    let empty = deltas.before(None);
    assert!(empty.deltas.is_empty() && empty.gaps.is_empty() && !empty.all_gaps);
}

#[test]
fn lifecycle_frames_map_only_allowlisted_items() {
    let started = frame(json!({"method":"turn/started","params":
        {"threadId":"t","turn":{"id":"u","status":"inProgress"}}}));
    assert_eq!(
        event(&started),
        Some(Event {
            turn_id: Some("u".into()),
            status: Some("inProgress".into()),
            ..Event::new(Kind::TurnStarted)
        })
    );
    let completed = frame(json!({"method":"turn/completed","params":
        {"threadId":"t","turn":{"id":"u","status":"completed"}}}));
    assert_eq!(event(&completed).unwrap().kind, Some(Kind::TurnCompleted));
    let command = |method: &str, extra: Value| {
        let mut item = json!({"type":"commandExecution","id":"c","command":"cargo test",
            "cwd":"/w","commandActions":[],"status":"inProgress"});
        item.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        frame(json!({"method": method, "params": {"threadId":"t","turnId":"u","item": item}}))
    };
    let begun = event(&command("item/started", json!({}))).unwrap();
    assert_eq!(
        (
            begun.kind,
            begun.tool.as_deref(),
            begun.text.as_deref(),
            begun.status
        ),
        (
            Some(Kind::ToolStarted),
            Some("command"),
            Some("cargo test"),
            None
        )
    );
    let ended = event(&command(
        "item/completed",
        json!({"status":"failed","exitCode":101}),
    ))
    .unwrap();
    assert_eq!(
        (ended.kind, ended.text.as_deref(), ended.status.as_deref()),
        (Some(Kind::ToolCompleted), Some("exit 101"), Some("failed"))
    );
    let argv = event(&command(
        "item/started",
        json!({"command":["git","status"]}),
    ))
    .unwrap();
    assert_eq!(argv.text.as_deref(), Some("git status"));
    let files = frame(
        json!({"method":"item/started","params":{"threadId":"t","turnId":"u",
        "item":{"type":"fileChange","id":"f","status":"inProgress",
            "changes":[{"path":"a.rs"},{"path":"b.rs"}]}}}),
    );
    assert_eq!(event(&files).unwrap().text.as_deref(), Some("a.rs, b.rs"));
    let tool = frame(
        json!({"method":"item/started","params":{"threadId":"t","turnId":"u",
        "item":{"type":"dynamicToolCall","id":"d","tool":"hive_send","arguments":{},"status":"inProgress"}}}),
    );
    assert_eq!(event(&tool).unwrap().tool.as_deref(), Some("hive_send"));
    let mcp = frame(
        json!({"method":"item/started","params":{"threadId":"t","turnId":"u",
        "item":{"type":"mcpToolCall","id":"m","server":"docs","tool":"search","arguments":{},"status":"inProgress"}}}),
    );
    assert_eq!(event(&mcp).unwrap().tool.as_deref(), Some("docs/search"));
}

#[test]
fn echoes_assistant_text_and_unknown_items_are_not_live_items() {
    // User echoes, assistant text and unknown items are durable-only or dropped.
    for item in [
        json!({"type":"userMessage","id":"x","content":[{"type":"text","text":"a whole joined reply"}]}),
        json!({"type":"agentMessage","id":"x","text":"answer"}),
        json!({"type":"hookPrompt","id":"x","fragments":[]}),
        json!({"type":"somethingNew","id":"x"}),
    ] {
        for method in ["item/started", "item/completed"] {
            let value = frame(
                json!({"method": method, "params": {"threadId":"t","turnId":"u","item": item}}),
            );
            assert_eq!(event(&value), None, "{item}");
        }
    }
    assert_eq!(
        event(&frame(
            json!({"method":"model/rerouted","params":{"threadId":"t"}})
        )),
        None
    );
}

mod original;

#[test]
fn typed_live_projection_matches_partial_and_unknown_native_frames() {
    let malformed = [
        Value::Null,
        json!(true),
        json!(5),
        json!(5.0),
        json!("text"),
        json!([]),
        json!({}),
        json!([null, "cmd", 2, "arg"]),
        json!([{"path":"one"},null,{"path":2},{"path":"two"}]),
    ];
    for method in [
        AGENT_MESSAGE_DELTA,
        COMMAND_OUTPUT_DELTA,
        REASONING_SUMMARY_DELTA,
        RAW_REASONING_DELTA,
        "turn/started",
        "turn/completed",
        "item/started",
        "item/completed",
        "future",
    ] {
        for kind in [
            "commandExecution",
            "fileChange",
            "dynamicToolCall",
            "mcpToolCall",
            "future",
        ] {
            let raw = json!({"method":method,"params":{"threadId":"thread","turnId":"turn",
                "itemId":"item","delta":"text","turn":{"id":"turn","status":"future"},
                "item":{"type":kind,"id":"item","status":"done","command":["cmd","arg"],
                    "changes":[{"path":"one"}],"tool":"tool","server":"server","exitCode":0}}});
            for path in [
                "/method",
                "/params/threadId",
                "/params/turnId",
                "/params/itemId",
                "/params/delta",
                "/params/turn",
                "/params/turn/id",
                "/params/turn/status",
                "/params/item/type",
                "/params/item/command",
                "/params/item/changes",
                "/params/item/tool",
                "/params/item/server",
                "/params/item/exitCode",
                "/params/item",
                "/params",
            ] {
                for value in &malformed {
                    let mut changed = raw.clone();
                    *changed.pointer_mut(path).unwrap() = value.clone();
                    assert_eq!(delta_kind(&changed), original::delta_kind(&changed));
                    assert_eq!(
                        delta(Kind::Text, &changed),
                        original::delta(Kind::Text, &changed)
                    );
                    let frame = IncomingFrame::parse(changed);
                    assert_eq!(
                        event(&frame),
                        original::event(&frame),
                        "{method} {kind} {path}"
                    );
                }
            }
        }
    }
}
