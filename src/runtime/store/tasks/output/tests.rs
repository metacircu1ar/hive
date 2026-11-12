use super::*;
use std::error::Error as _;

fn invalid_without_source(encoded: &str) {
    let error = select(encoded).unwrap_err();
    assert_eq!(error.code(), "database_error");
    assert_eq!(error.message(), "Invalid saved message output.");
    assert!(error.source().is_none());
}

#[test]
fn selection_keeps_lenient_fields_and_map_only_artifact_references() {
    for (encoded, expected) in [
        (
            r#"{"text":"t","extra":[null,7]}"#,
            StoredOutput::Text("t".into()),
        ),
        (r#"{"text":""}"#, StoredOutput::Text(String::new())),
        (r#"{"text":" \t "}"#, StoredOutput::Text(" \t ".into())),
        (
            r#"{"text":"é\nquote\"\u0000"}"#,
            StoredOutput::Text("é\nquote\"\0".into()),
        ),
        (
            r#"{"artifact":["x"],"text":"t"}"#,
            StoredOutput::Text("t".into()),
        ),
        (
            r#"{"artifact":null,"text":"t"}"#,
            StoredOutput::Text("t".into()),
        ),
        (
            r#"{"artifact":{"id":7},"text":"t"}"#,
            StoredOutput::Text("t".into()),
        ),
        (
            r#"{"artifact":{"id":""},"text":"t"}"#,
            StoredOutput::Artifact(String::new()),
        ),
        (
            r#"{"artifact":{"id":"id"},"text":{}}"#,
            StoredOutput::Artifact("id".into()),
        ),
    ] {
        assert_eq!(select(encoded).unwrap(), expected);
    }
    for encoded in [
        "null",
        "false",
        "7",
        r#""t""#,
        r#"["t"]"#,
        "{}",
        r#"{"text":null}"#,
        r#"{"text":7}"#,
        r#"{"text":[]}"#,
        r#"{"text":{}}"#,
    ] {
        invalid_without_source(encoded);
    }
}

#[test]
fn duplicate_keys_keep_the_last_value_before_projection() {
    assert_eq!(
        select(r#"{"text":"first","text":"last","artifact":{"id":"old","id":null}}"#).unwrap(),
        StoredOutput::Text("last".into()),
    );
    assert_eq!(
        select(r#"{"artifact":{"id":null,"id":"last"},"text":7}"#).unwrap(),
        StoredOutput::Artifact("last".into()),
    );
}

#[test]
fn syntax_errors_keep_a_source_but_valid_wrong_shapes_do_not() {
    for encoded in ["{", r#"{"text":"x"} trailing"#] {
        let error = select(encoded).unwrap_err();
        assert_eq!(error.message(), "Invalid saved message output.");
        assert!(error.source().is_some());
    }
    for encoded in [r#"["t"]"#, "null", r#"{"artifact":{"id":7}}"#] {
        invalid_without_source(encoded);
    }
}

#[test]
fn artifact_text_does_not_follow_a_second_reference_or_accept_positional_arrays() {
    assert_eq!(
        text(r#"{"artifact":{"id":"not-an-id"},"text":"inner"}"#).unwrap(),
        "inner"
    );
    for encoded in [r#"["t"]"#, r#""t""#, "7", r#"{"artifact":{"id":"id"}}"#] {
        let error = text(encoded).unwrap_err();
        assert_eq!(error.message(), "Invalid saved message output.");
        assert!(error.source().is_none());
    }
}
