use super::*;
use serde_json::json;

#[test]
fn native_reply_decisions_preserve_malformed_siblings_and_raw_values() {
    let values = [
        Value::Null,
        json!(true),
        json!(1),
        json!("x"),
        json!([]),
        json!({}),
    ];
    let good = json!({"type":"control_response","unknown":{"opaque":[5]},
        "response":{"subtype":"success","request_id":"x","response":{
            "hookSpecificOutput":{"additionalContext":"context"}}}});
    for path in [
        "/type",
        "/response/subtype",
        "/response/request_id",
        "/response/response",
        "/response/response/hookSpecificOutput",
        "/response/response/hookSpecificOutput/additionalContext",
        "/response",
    ] {
        for replacement in &values {
            let mut raw = good.clone();
            *raw.pointer_mut(path).unwrap() = replacement.clone();
            let projection = Decision::read(&raw);
            assert_eq!(
                projection.successful_reply(),
                raw["type"] == "control_response" && raw["response"]["subtype"] == "success"
            );
            assert_eq!(
                projection.request_id(),
                raw["response"]["request_id"].as_str()
            );
            assert_eq!(
                projection.has_guidance(),
                raw["response"]["response"]["hookSpecificOutput"]["additionalContext"].is_string()
            );
        }
    }
    for raw in values {
        let projection = Decision::read(&raw);
        assert!(!projection.has_guidance());
        assert!(!projection.successful_reply());
        assert!(projection.request_id().is_none());
    }
}
