use super::*;

fn finish(f: &mut Fixture, turn: &str, usage: Value, failed: bool) {
    f.codec
        .receive(
            json!({"type":"result", "session_id":f.id,"user_message_uuid":turn,
        "subtype":if failed {"error_during_execution"} else {"success"},
        "is_error":failed,"terminal_reason":"completed","modelUsage":usage}),
        )
        .unwrap();
}
fn counters() -> Value {
    json!({"inputTokens":12,"outputTokens":8,"cacheReadInputTokens":4,
        "cacheCreationInputTokens":0,"costUSD":99,"contextWindow":200_000})
}
#[test]
fn native_usage_is_typed_excludes_money_and_precedes_only_its_matching_terminal_result() {
    for failed in [false, true] {
        let mut f = Fixture::new();
        let turn = f.turn();
        finish(
            &mut f,
            &turn,
            json!({"sonnet[1m]":counters(),"provider/model:revision@alias":counters()}),
            failed,
        );
        let usage = f.codec.pop().unwrap();
        assert_eq!(usage["method"], "hive/claude/modelUsage");
        assert_eq!(usage["params"]["threadId"], f.id);
        assert_eq!(usage["params"]["turnId"], turn);
        assert_eq!(
            usage["params"]["outcome"],
            if failed { "failed" } else { "completed" }
        );
        assert_eq!(usage["params"]["models"]["sonnet[1m]"]["input_tokens"], 12);
        assert!(!usage.to_string().contains("costUSD"));
        assert!(!usage.to_string().contains("contextWindow"));
        assert_eq!(f.codec.pop().unwrap()["method"], "turn/completed");
        assert!(f.codec.pop().is_none());
    }
}
#[test]
fn malformed_optional_usage_is_dropped_as_a_whole_without_changing_the_turn() {
    for usage in [
        Value::Null,
        json!([]),
        json!({"sonnet":counters(),"invalid name":counters()}),
        json!({"sonnet":counters(),"bad":{"inputTokens":-1}}),
    ] {
        let mut f = Fixture::new();
        let turn = f.turn();
        finish(&mut f, &turn, usage, false);
        assert_eq!(f.codec.pop().unwrap()["method"], "turn/completed");
        assert!(f.codec.pop().is_none());
    }
}
#[test]
fn a_rejected_session_or_wrong_result_cannot_emit_agent_usage() {
    let mut f = Fixture::new();
    let turn = f.turn();
    assert!(
        f.codec
            .receive(json!({"type":"result","session_id":f.id,
        "user_message_uuid":format!("wrong-{turn}"),"subtype":"success","is_error":false,
        "terminal_reason":"completed","modelUsage":{"sonnet":counters()}}))
            .is_err()
    );
    assert!(f.codec.pop().is_none());
}
