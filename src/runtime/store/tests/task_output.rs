use super::*;
use crate::runtime::domain::OutputSelection;
use serde_json::Value;
use std::error::Error as _;

struct Fixture {
    store: Store,
    agent: Agent,
    _temp: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let temp = crate::runtime::test_directory();
        let store = Store::open(&temp.path().join("db")).unwrap();
        let agent = agent("worker", None);
        store.save(&agent).unwrap();
        store.enqueue("task", "human", &agent.id, "input").unwrap();
        store.track_call("task").unwrap();
        Self {
            store,
            agent,
            _temp: temp,
        }
    }
    fn output(&self, raw: Option<&str>, final_answer: bool) {
        self.store
            .execute_cached(
                "UPDATE call_results SET output=?1,output_final=?2 WHERE message_id='task'",
                rusqlite::params![raw, final_answer],
            )
            .unwrap();
    }
    fn artifact(&self, value: &Value) -> String {
        self.store
            .publish_artifact(value, api::encoded_len(value), Some(&self.agent.id), None)
            .unwrap()
            .0
    }
}

#[test]
fn artifact_selection_ignores_malformed_inline_text_without_losing_the_reference() {
    let fixture = Fixture::new();
    let id = fixture.artifact(&json!({"text":"artifact text"}));
    for inline in [
        None,
        Some(Value::Null),
        Some(json!(7)),
        Some(json!({})),
        Some(json!([])),
    ] {
        let mut raw = json!({"artifact":{"id":id}});
        if let Some(inline) = inline {
            raw["text"] = inline;
        }
        fixture.output(Some(&raw.to_string()), true);
        assert_eq!(
            fixture.store.selected_text("task").unwrap(),
            ("artifact text".into(), OutputSelection::FinalAnswer),
        );
    }
    fixture.output(Some(r#"{"artifact":["x"],"text":"inline"}"#), false);
    assert_eq!(fixture.store.selected_text("task").unwrap().0, "inline");
}

#[test]
fn artifact_lookup_and_missing_file_errors_win_over_inline_text() {
    let fixture = Fixture::new();
    for (id, expected) in [
        (uuid::Uuid::new_v4().to_string(), "unknown_artifact"),
        (String::new(), "invalid_artifact"),
        ("not-an-id".into(), "invalid_artifact"),
    ] {
        fixture.output(
            Some(&json!({"artifact":{"id":id},"text":"fallback"}).to_string()),
            false,
        );
        assert_eq!(
            fixture.store.selected_text("task").unwrap_err().code(),
            expected
        );
    }
    let id = fixture.artifact(&json!({"text":"removed"}));
    unlink_artifacts(&fixture.store.artifacts_dir, std::slice::from_ref(&id)).unwrap();
    fixture.output(
        Some(&json!({"artifact":{"id":id},"text":"fallback"}).to_string()),
        false,
    );
    assert_eq!(
        fixture.store.selected_text("task").unwrap_err().code(),
        "artifact_unavailable"
    );
}

#[test]
fn artifact_payload_is_map_only_and_does_not_follow_a_second_reference() {
    let fixture = Fixture::new();
    for body in [
        json!(["positional"]),
        json!("scalar"),
        json!({"artifact":{"id":"second"}}),
    ] {
        let id = fixture.artifact(&body);
        fixture.output(
            Some(&json!({"artifact":{"id":id},"text":"fallback"}).to_string()),
            false,
        );
        let error = fixture.store.selected_text("task").unwrap_err();
        assert_eq!(error.message(), "Invalid saved message output.");
        assert!(error.source().is_none());
    }
    let id = fixture.artifact(&json!({"artifact":{"id":"not-an-id"},"text":"inner"}));
    fixture.output(Some(&json!({"artifact":{"id":id}}).to_string()), false);
    assert_eq!(fixture.store.selected_text("task").unwrap().0, "inner");
    let mut completed = fixture.agent.clone();
    completed.result = Some("stale fallback".into());
    completed.result_read_target = Some(format!("artifact:{id}"));
    assert_eq!(fixture.store.completion_text(&completed).unwrap(), "inner");
}

#[test]
fn saved_text_preserves_selection_null_rows_and_parse_error_sources() {
    let fixture = Fixture::new();
    let error = fixture.store.selected_text("missing").unwrap_err();
    assert_eq!(error.message(), "Invalid saved message output.");
    assert!(error.source().is_none());
    fixture.output(None, true);
    assert_eq!(
        fixture.store.selected_text("task").unwrap(),
        (String::new(), OutputSelection::NoText)
    );
    for final_answer in [false, true] {
        fixture.output(Some(r#"{"text":""}"#), final_answer);
        assert_eq!(
            fixture.store.selected_text("task").unwrap().1,
            OutputSelection::NoText
        );
        fixture.output(Some(r#"{"text":" \t "}"#), final_answer);
        let expected = if final_answer {
            OutputSelection::FinalAnswer
        } else {
            OutputSelection::LastAssistantMessage
        };
        assert_eq!(
            fixture.store.selected_text("task").unwrap(),
            (" \t ".into(), expected)
        );
    }
    for raw in ["{", r#"["t"]"#, r#"{"text":null}"#] {
        fixture.output(Some(raw), false);
        let error = fixture.store.selected_text("task").unwrap_err();
        assert_eq!(error.message(), "Invalid saved message output.");
        assert_eq!(error.source().is_some(), raw == "{");
    }
}

#[test]
fn completion_preserves_inline_fallback_and_large_artifact_chunking() {
    let fixture = Fixture::new();
    let mut completed = fixture.agent.clone();
    assert_eq!(fixture.store.completion_text(&completed).unwrap(), "");
    completed.result = Some("inline\0text".into());
    assert_eq!(
        fixture.store.completion_text(&completed).unwrap(),
        "inline\0text"
    );
    completed.result_read_target = Some("agent:worker".into());
    let error = fixture.store.completion_text(&completed).unwrap_err();
    assert_eq!(error.message(), "Invalid saved message output.");
    assert!(error.source().is_none());
    let text = "é\n\0quoted\"".repeat(20_000);
    let id = fixture.artifact(&json!({"text":text}));
    fixture.output(Some(&json!({"artifact":{"id":id}}).to_string()), false);
    assert_eq!(fixture.store.selected_text("task").unwrap().0, text);
    completed.result_read_target = Some(format!("artifact:{id}"));
    assert_eq!(fixture.store.completion_text(&completed).unwrap(), text);
}
