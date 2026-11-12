use super::*;
use std::error::Error as _;

fn root(content: &str) -> String {
    format!(
        concat!(
            "{{\"type\":\"user\",\"uuid\":\"12345678-abcd-4234-8234-123456789abc\",",
            "\"parentUuid\":null,\"message\":{{\"role\":\"user\",\"content\":{content}}}}}"
        ),
        content = content
    )
}

#[test]
fn headers_keep_absent_null_wrong_types_and_validation_source_order() {
    for raw in ["null", "[]", "7", "\"x\"", "{}", r#"{"type":7}"#] {
        assert!(decode(raw.as_bytes()).unwrap().is_empty());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let good = row(&id, None, "text");
    for (key, bad) in [
        ("isSidechain", json!(null)),
        ("isSidechain", json!(7)),
        ("parentUuid", json!([])),
        ("message", json!([])),
    ] {
        let mut value = good.clone();
        value[key] = bad;
        let error = decode(value.to_string().as_bytes()).unwrap_err();
        assert!(error.source().is_none());
    }
    let mut value = good;
    value["uuid"] = json!("invalid");
    value["message"] = json!(7);
    assert!(
        decode(value.to_string().as_bytes())
            .unwrap_err()
            .source()
            .is_some()
    );
    value["isSidechain"] = json!(true);
    assert!(decode(value.to_string().as_bytes()).unwrap().is_empty());
}

#[test]
fn main_messages_require_matching_role_and_string_or_array_content() {
    for content in [r#""é\n\u0000""#, "[]", r#"[null,7,{"unknown":"field"}]"#] {
        assert_eq!(decode(root(content).as_bytes()).unwrap().len(), 1);
    }
    for content in ["null", "7", "true", "{}"] {
        assert!(decode(root(content).as_bytes()).is_err());
    }
    let mut value: Value = serde_json::from_str(&root("[]")).unwrap();
    for role in [json!(null), json!("assistant"), json!([])] {
        value["message"]["role"] = role;
        assert!(decode(value.to_string().as_bytes()).is_err());
    }
    value["message"]["role"] = json!("user");
    value.as_object_mut().unwrap().remove("parentUuid");
    assert!(decode(value.to_string().as_bytes()).is_err());
}

#[test]
fn history_preserves_exact_uuid_spellings_in_chain_and_equality() {
    let uuid = uuid::Uuid::parse_str("12345678-abcd-4234-8234-123456789abc").unwrap();
    let lower = uuid.to_string();
    let forms = [
        lower.to_uppercase(),
        uuid.simple().to_string(),
        uuid.braced().to_string(),
        uuid.urn().to_string(),
    ];
    for form in forms {
        let original = rows(&[row(&lower, None, "text")]);
        let variant = rows(&[row(&form, None, "text")]);
        assert_ne!(original, variant);
        let child = row(&uuid::Uuid::new_v4().to_string(), Some(&form), "child");
        assert!(decode(format!("{}\n{child}", row(&lower, None, "text")).as_bytes()).is_err());
        let distinct = rows(&[row(&lower, None, "text"), row(&form, Some(&lower), "child")]);
        assert_eq!(distinct.len(), 2);
        assert_eq!(distinct[1].id, form);
    }
}

#[test]
fn dialogue_comparison_keeps_opaque_message_values_and_ignores_top_level_noise() {
    let first = root(r#"[{"number":1.0,"text":"é\n\u0000","extra":true}]"#);
    let reordered = concat!(
        "{ \"unused\":7, \"parentUuid\": null, \"message\": {",
        "\"content\":[{\"extra\":true,\"text\":\"é\\n\\u0000\",\"number\":1e0}],\"role\":\"user\"},",
        "\"uuid\":\"12345678-abcd-4234-8234-123456789abc\", \"type\":\"user\" }"
    );
    assert_eq!(
        decode(first.as_bytes()).unwrap(),
        decode(reordered.as_bytes()).unwrap()
    );
    assert_ne!(
        decode(root("[1]").as_bytes()).unwrap(),
        decode(root("[1.0]").as_bytes()).unwrap()
    );
    assert_eq!(
        decode(root("[1.0]").as_bytes()).unwrap(),
        decode(root("[1e0]").as_bytes()).unwrap()
    );
    let mut extra: Value = serde_json::from_str(&first).unwrap();
    extra["message"]["unknown"] = json!(7);
    assert_ne!(
        decode(first.as_bytes()).unwrap(),
        decode(extra.to_string().as_bytes()).unwrap()
    );
}

#[test]
fn duplicate_keys_are_resolved_last_before_header_projection() {
    let raw = root("[]")
        .replace(
            "\"parentUuid\":null",
            "\"parentUuid\":7,\"parentUuid\":null",
        )
        .replace("\"type\":\"user\"", "\"type\":7,\"type\":\"user\"");
    assert_eq!(
        decode(raw.as_bytes()).unwrap(),
        decode(root("[]").as_bytes()).unwrap()
    );
    let rejected = root("[]").replace(
        "\"parentUuid\":null",
        "\"parentUuid\":null,\"parentUuid\":7",
    );
    assert!(decode(rejected.as_bytes()).is_err());
}

#[test]
fn record_limit_counts_ignored_rows_and_checks_before_parsing_the_extra_row() {
    let mut bytes = "{}\n".repeat(MAX_RECORDS).into_bytes();
    assert!(decode(&bytes).unwrap().is_empty());
    bytes.extend_from_slice(b"{");
    let error = decode(&bytes).unwrap_err();
    assert!(error.source().is_none());
}
