//! Public malformed-value cases shared by the native transports.
use super::*;

pub(super) fn invalid_value_actions() -> (Vec<Value>, Vec<::comb::Error>) {
    let cases = [
        (
            "slug",
            json!("BAD SECRET INPUT"),
            "a page slug: lowercase letters, digits, hyphens or underscores in \
                slash-separated components; at most 240 bytes total and 100 per \
                component",
        ),
        (
            "request_id",
            json!(""),
            "a request ID of 1..128 UTF-8 bytes without control characters; reuse identical \
                arguments and ID on \
                retries",
        ),
        (
            "run",
            json!("\n"),
            "a run name of 1..512 UTF-8 bytes without control characters",
        ),
        (
            "expected_hash",
            json!("BAD SECRET INPUT"),
            "64 lowercase hexadecimal characters from the current page hash",
        ),
    ];
    let mut actions: Vec<Value> = cases
        .iter()
        .map(|(field, value, _)| {
            let mut args = json!({"slug":"concepts/input","text":"New evidence","source":"test",
            "request_id":"input-original"});
            args[*field] = value.clone();
            json!({"name":"comb_append","arguments":args})
        })
        .collect();
    actions.push(
        json!({"name":"comb_search","arguments":{"query":"input","after":"BAD SECRET INPUT"}}),
    );
    let mut expected: Vec<_> = cases
        .into_iter()
        .map(|(field, _, expected)| {
            hive_input::InputError::new(
                hive_input::Code::InvalidValue,
                "Invalid write-operation input.",
                format!("/arguments/{field}"),
                expected,
                vec!["comb_append {}".into()],
            )
            .unwrap()
            .into()
        })
        .collect();
    expected.push(
        hive_input::InputError::new(
            hive_input::Code::InvalidValue,
            "Invalid tool or retrieval input.",
            "/arguments/after",
            "an RFC3339 timestamp, with after no later than before",
            vec!["comb_search {\"query\":\"architecture\"}".into()],
        )
        .unwrap()
        .into(),
    );
    (actions, expected)
}
