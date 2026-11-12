use super::*;
use serde_json::json;

#[test]
fn typed_claude_identity_tuple_matches_the_original_canonical_tree_and_pinned_bytes() {
    for (native, expected) in [
        (json!(42), r#"["claude","turn",["control",42]]"#),
        (
            json!("quoted\"é\0"),
            "[\"claude\",\"turn\",[\"control\",\"quoted\\\"é\\u0000\"]]",
        ),
        (json!(null), r#"["claude","turn",["control",null]]"#),
        (
            json!(u64::MAX),
            r#"["claude","turn",["control",18446744073709551615]]"#,
        ),
        (
            json!([null, true]),
            r#"["claude","turn",["control",[null,true]]]"#,
        ),
        (
            json!({"z":1,"a":2}),
            r#"["claude","turn",["control",{"a":2,"z":1}]]"#,
        ),
    ] {
        let tuple = ("control", &native);
        let previous = crate::api::value(tuple);
        let expected = format!("{:x}", Sha256::digest(expected.as_bytes()));
        assert_eq!(request_id("claude", "turn", &tuple), expected);
        assert_eq!(
            request_id("claude", "turn", &tuple),
            request_id("claude", "turn", &previous)
        );
    }
    assert_ne!(
        request_id("execution", "turn", &json!(1)),
        request_id("execution", "turn", &json!(1.0))
    );
    assert_eq!(
        request_id("execution", "turn", &json!({"z":1,"a":2})),
        request_id("execution", "turn", &json!({"a":2,"z":1}))
    );
}
