use super::*;
use serde_json::json;

#[test]
fn canonical_text_pins_quoted_strings_numbers_and_nested_key_order() {
    for (value, expected) in [
        (json!("é\n\"\0"), "\"é\\n\\\"\\u0000\""),
        (json!(u64::MAX), "18446744073709551615"),
        (json!(-0.0), "-0.0"),
        (
            json!([null,true,{"z":1,"a":2}]),
            r#"[null,true,{"a":2,"z":1}]"#,
        ),
    ] {
        assert_eq!(json_text(&value), expected);
        assert_eq!(json_text(&value), crate::api::value(value).to_string());
    }
}

#[test]
fn canonical_text_keeps_struct_order_float_widening_and_flatten_collision_semantics() {
    #[derive(Serialize)]
    struct Unsorted {
        z: u64,
        a: &'static str,
    }
    #[derive(Serialize)]
    struct Float {
        value: f32,
    }
    #[derive(Serialize)]
    struct Collision {
        value: u64,
        #[serde(flatten)]
        rest: std::collections::BTreeMap<String, u64>,
    }
    assert_eq!(json_text(Unsorted { z: 1, a: "x" }), r#"{"a":"x","z":1}"#);
    assert_eq!(
        json_text(Float { value: 0.1 }),
        r#"{"value":0.10000000149011612}"#
    );
    assert_eq!(
        json_text(Collision {
            value: 1,
            rest: [("value".into(), 2)].into()
        }),
        r#"{"value":2}"#
    );
}

#[test]
fn native_cleanup_and_warning_text_keep_full_canonical_bytes_and_readback() {
    use crate::runtime::api::{NativeCleanup, NativeWarningReason};
    let warning = NativeWarningReason::SharedSession;
    assert_eq!(json_text(warning), r#""shared_session""#);
    assert_eq!(
        serde_json::from_str::<NativeWarningReason>(&json_text(warning)).unwrap(),
        warning
    );
    let mut cleanup = NativeCleanup::new(false);
    cleanup.warn("agent", warning);
    cleanup.native_bytes_deleted = u64::MAX;
    assert_eq!(json_text(&cleanup), crate::api::value(&cleanup).to_string());
    assert_eq!(
        json_text(&cleanup),
        concat!(
            "{\"native_bytes_deleted\":18446744073709551615,\"native_history\":\"partial\",",
            "\"native_preview\":[],\"native_preview_truncated\":false,\"native_sessions_absent\":0,",
            "\"native_sessions_deleted\":0,\"native_sessions_retained\":0,\"native_warning_count\":1,",
            "\"native_warnings\":[{\"agent_id\":\"agent\",\"reason\":\"shared_session\"}]}"
        )
    );
}

#[test]
fn optional_join_output_text_keeps_literal_keys_and_escaping() {
    use crate::runtime::api::joins::JoinOutput;
    for read_target in [None, Some("artifact:quoted\"é".into())] {
        let output = JoinOutput {
            text: "é\n\"\0".into(),
            read_target,
        };
        assert_eq!(json_text(&output), crate::api::value(&output).to_string());
    }
    assert_eq!(
        json_text(JoinOutput {
            text: String::new(),
            read_target: None
        }),
        r#"{"text":""}"#
    );
}
