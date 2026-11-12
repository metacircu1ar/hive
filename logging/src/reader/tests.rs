use super::*;
use serde_json::json;

#[test]
fn admission_and_filtering_preserve_raw_unknown_fields_and_bad_optional_fields() {
    for (value, valid) in [
        (
            json!({"event":"", "agent_id":false, "level":17, "future":{"x":false}}),
            true,
        ),
        (
            json!({"event":"future", "agent_id":"agent", "level":"INFO"}),
            true,
        ),
        (json!({"event":17}), false),
        (json!({}), false),
        (json!(["future", "agent", "INFO"]), false),
        (json!(null), false),
    ] {
        let parsed = parse_record(value.to_string().as_bytes());
        assert_eq!(parsed.is_some(), valid);
        if let Some(parsed) = parsed {
            assert_eq!(parsed, value);
            assert!(Filter::default().accepts(&parsed));
        }
    }
    assert!(parse_record(&vec![b' '; RECORD_BYTES + 1]).is_none());
}

#[test]
fn log_filters_keep_their_independent_optional_fields_and_level_order() {
    let value = json!({"event":"future", "agent_id":"agent", "level":"INFO"});
    for (filter, expected) in filters() {
        assert_eq!(filter.accepts(&value), expected);
    }
    let bad = json!({"event":"future", "agent_id":17, "level":"UNKNOWN"});
    assert!(Filter::default().accepts(&bad));
    assert!(
        !Filter {
            level: Some(tracing::Level::DEBUG),
            ..Filter::default()
        }
        .accepts(&bad)
    );
    assert!(
        !Filter {
            agent_id: Some("17".into()),
            ..Filter::default()
        }
        .accepts(&bad)
    );
}

fn filters() -> [(Filter, bool); 6] {
    [
        (
            Filter {
                event: Some("future".into()),
                ..Filter::default()
            },
            true,
        ),
        (
            Filter {
                event: Some("other".into()),
                ..Filter::default()
            },
            false,
        ),
        (
            Filter {
                agent_id: Some("agent".into()),
                ..Filter::default()
            },
            true,
        ),
        (
            Filter {
                agent_id: Some("other".into()),
                ..Filter::default()
            },
            false,
        ),
        (
            Filter {
                level: Some(tracing::Level::DEBUG),
                ..Filter::default()
            },
            true,
        ),
        (
            Filter {
                level: Some(tracing::Level::ERROR),
                ..Filter::default()
            },
            false,
        ),
    ]
}
