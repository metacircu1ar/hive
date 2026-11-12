//! Native output regressions.
use super::*;

#[test]
fn assistant_split_frames_keep_raw_order_and_summarize_only_at_boundaries() {
    let mut f = Fixture::new();
    f.turn();
    f.assistant(
        Some("native-1"),
        "claude-sonnet-5",
        json!([
            {"type":"text","text":"first"},
            {"type":"tool_use","id":"read-1","name":"Read","input":{"file_path":"foo"}}
        ]),
    );
    assert_eq!(f.codec.pop().unwrap()["params"]["item"]["text"], "first");
    assert_eq!(
        f.codec.pop().unwrap()["params"]["item"]["type"],
        "dynamicToolCall"
    );
    f.assistant(
        None,
        "<synthetic>",
        json!([{"type":"text","text":"local notice"}]),
    );
    assert_eq!(
        f.codec.pop().unwrap()["params"]["item"]["verifiedModel"],
        false
    );
    f.assistant(
        Some("native-1"),
        "claude-sonnet-5",
        json!([
            {"type":"thinking","thinking":"not output"},{"type":"text","text":"second"}
        ]),
    );
    assert_eq!(f.codec.pop().unwrap()["params"]["item"]["text"], "second");
    assert!(f.codec.pop().is_none());
    f.assistant(
        Some("native-2"),
        "claude-sonnet-5",
        json!([{"type":"text","text":"new"}]),
    );
    let summary = f.codec.pop().unwrap();
    assert_eq!(summary["params"]["item"]["text"], "first\nsecond");
    assert_eq!(summary["params"]["item"]["aggregation"], "native_message");
    assert_eq!(summary["params"]["item"]["nativeMessageId"], "native-1");
    assert_eq!(f.codec.pop().unwrap()["params"]["item"]["text"], "new");
    f.assistant(
        None,
        "claude-sonnet-5",
        json!([{"type":"text","text":"idless-a"}]),
    );
    f.assistant(
        None,
        "claude-sonnet-5",
        json!([{"type":"text","text":"idless-b"}]),
    );
    for text in ["idless-a", "idless-b"] {
        assert_eq!(f.codec.pop().unwrap()["params"]["item"]["text"], text);
    }
    assert!(f.codec.pop().is_none());
}

#[test]
fn assistant_aggregate_bound_fails_explicitly_without_cumulative_summaries() {
    let mut f = Fixture::new();
    f.turn();
    let large = "x".repeat(1024 * 1024);
    for _ in 0..3 {
        f.assistant(
            Some("large"),
            "claude-sonnet-5",
            json!([{"type":"text","text":large}]),
        );
        let raw = f.codec.pop().unwrap();
        assert_eq!(
            raw["params"]["item"]["text"].as_str().unwrap().len(),
            large.len()
        );
        assert!(raw["params"]["item"].get("aggregation").is_none());
        assert!(f.codec.pop().is_none());
    }
    let error = f
        .codec
        .receive(json!({"type":"assistant","session_id":f.id,"message":{
        "id":"large","model":"claude-sonnet-5","content":[{"type":"text","text":large}]}}))
        .unwrap_err();
    assert_eq!(error.code(), "output_limit");
    assert!(f.codec.pop().is_none());
}

#[test]
fn assistant_text_blocks_are_combined_in_native_order_with_model_provenance() {
    let mut f = Fixture::new();
    f.turn();
    f.codec
        .receive(json!({"type":"assistant","session_id":f.id,"message":{
        "model":"claude-sonnet-5","content":[{"type":"text","text":"first"},
        {"type":"thinking","thinking":"not output"},{"type":"text","text":"second"}]}}))
        .unwrap();
    for text in ["first", "second", "first\nsecond"] {
        let frame = f.codec.pop().unwrap();
        assert_eq!(frame["params"]["item"]["text"], text);
        assert_eq!(frame["params"]["item"]["verifiedModel"], true);
    }
    assert!(f.codec.pop().is_none());
    f.codec
        .receive(json!({"type":"assistant","session_id":f.id,"message":{
        "model":"<synthetic>","content":[{"type":"text","text":"local notice"}]}}))
        .unwrap();
    assert_eq!(
        f.codec.pop().unwrap()["params"]["item"]["verifiedModel"],
        false
    );
}

#[test]
fn assistant_summary_precedes_terminal_outcomes_and_resets_between_turns() {
    for outcome in [
        TerminalCase::Completed,
        TerminalCase::Interrupted,
        TerminalCase::Failed,
    ] {
        let reason = outcome.wire();
        let succeeded = matches!(outcome, TerminalCase::Completed);
        let mut f = Fixture::new();
        for text in ["first turn", "second turn"] {
            let turn = f.turn();
            for fragment in [text, "tail"] {
                f.assistant(
                    Some("reused-native-id"),
                    "claude-sonnet-5",
                    json!([{"type":"text","text":fragment}]),
                );
                assert_eq!(f.codec.pop().unwrap()["params"]["item"]["text"], fragment);
                assert!(f.codec.pop().is_none());
            }
            f.codec
                .receive(json!({"type":"result","session_id":f.id,
                "subtype":if succeeded {"success"} else {"error_during_execution"},
                "is_error":!succeeded,"terminal_reason":reason,"user_message_uuid":turn}))
                .unwrap();
            let summary = f.codec.pop().unwrap();
            assert_eq!(summary["params"]["item"]["text"], format!("{text}\ntail"));
            assert_eq!(summary["params"]["turnId"], turn);
            let terminal = f.codec.pop().unwrap();
            assert_eq!(terminal["method"], "turn/completed");
            assert_eq!(terminal["params"]["turn"]["status"], outcome.status());
            assert!(f.codec.pop().is_none());
        }
    }
}

#[test]
fn assistant_after_terminal_cannot_replace_output_while_shell_cleanup_is_pending() {
    let mut f = Fixture::new();
    let turn = f.turn();
    let token = f.codec.guard.expect().unwrap();
    for text in ["first", "second"] {
        f.assistant(
            Some("native"),
            "claude-sonnet-5",
            json!([{"type":"text","text":text}]),
        );
        f.codec.pop().unwrap();
    }
    f.codec
        .receive(
            json!({"type":"result","session_id":f.id,"subtype":"success",
        "is_error":false,"terminal_reason":"completed","user_message_uuid":turn}),
        )
        .unwrap();
    assert_eq!(
        f.codec.pop().unwrap()["params"]["item"]["text"],
        "first\nsecond"
    );
    assert!(f.codec.pop().is_none(), "shell guard must delay completion");
    assert!(!f.codec.quiescent());
    assert!(f.codec.receive(json!({"type":"assistant","session_id":f.id,"message":{
        "id":"late","model":"claude-sonnet-5","content":[{"type":"text","text":"late overwrite"}]}})).is_err());
    assert!(f.codec.pop().is_none());
    f.codec.guard.revoke(&token);
    f.codec.settle();
    assert_eq!(f.codec.pop().unwrap()["method"], "turn/completed");
    assert!(f.codec.pop().is_none());
}
