use super::*;
use crate::{error::Error, native_sandbox::Outcome};
use serde_json::json;

fn core(value: Value, metadata: api::ControlReply) -> Output {
    Output::core(api::Response::Recorded(value), metadata)
}

#[test]
fn acknowledgement_and_error_envelopes_keep_literal_canonical_bytes() {
    let reply = Output::core(
        api::Response::from(api::SpawnResponse {
            agent_id: "a".into(),
            message_id: "m".into(),
        }),
        api::ControlReply::default(),
    );
    assert_eq!(
        encode(Ok(reply), None).to_string(),
        r#"{"agent_id":"a","message_id":"m","status":"accepted"}"#
    );
    assert_eq!(
        encode(
            Err(Error::invalid_request("invalid fixture")),
            Some("request".into())
        )
        .to_string(),
        concat!(
            r#"{"error":{"code":"invalid_request","message":"invalid fixture"},"#,
            r#""request_id":"request"}"#,
        )
    );
}

#[test]
fn recorded_wire_trees_preserve_metadata_precedence_nulls_and_payload_storage() {
    let value = json!({
        "text": "x".repeat(4096), "request_id":"old", "cleanup_pending":true,
        "z": {"z":null,"a":1},
    });
    let allocation = value["text"].as_str().unwrap().as_ptr();
    let expected = json!({
        "text": "x".repeat(4096), "request_id":"new", "cleanup_pending":false,
        "z": {"a":1,"z":null},
    });
    let reply = core(
        value,
        api::ControlReply {
            cleanup_pending: Some(false),
            request_id: Some("inner".into()),
            ..Default::default()
        },
    );
    let actual = encode(Ok(reply), Some("new".into()));
    assert_eq!(actual.to_string(), expected.to_string());
    assert_eq!(actual["text"].as_str().unwrap().as_ptr(), allocation);
    let inner = core(
        json!({"request_id":"old"}),
        api::ControlReply {
            request_id: Some("inner".into()),
            ..Default::default()
        },
    );
    assert_eq!(encode(Ok(inner), None), json!({"request_id":"inner"}));
}

#[test]
fn undecorated_recorded_scalars_and_arrays_are_returned_without_copying() {
    for value in [
        Value::Null,
        json!(true),
        json!(17),
        json!([null, {"z":2,"a":1}]),
    ] {
        let expected = value.to_string();
        assert_eq!(
            encode(Ok(core(value, api::ControlReply::default())), None).to_string(),
            expected
        );
    }
    let value = Value::String("é\n\"quoted\"".repeat(1024));
    let allocation = value.as_str().unwrap().as_ptr();
    let actual = encode(Ok(core(value, api::ControlReply::default())), None);
    assert_eq!(actual.as_str().unwrap().as_ptr(), allocation);
}

#[test]
#[should_panic(expected = "control replies are objects")]
fn decorated_recorded_scalar_keeps_the_existing_object_precondition() {
    encode(
        Ok(core(Value::Null, api::ControlReply::default())),
        Some("key".into()),
    );
}

#[test]
fn tree_attach_live_and_probe_payloads_keep_flattened_shapes() {
    let recorded = || api::Response::Recorded(json!({"z":null,"agents":[]}));
    let tree = api::TreeReply {
        version: live::TreeVersion {
            epoch: 1,
            revision: 2,
        },
        tree: recorded(),
    };
    assert_eq!(
        encode(Ok(Output::plain(tree)), None).to_string(),
        r#"{"agents":[],"version":{"epoch":1,"revision":2},"z":null}"#,
    );
    let attach = api::AttachReply {
        live: live::Cursor {
            epoch: 1,
            generation: 2,
            seq: 3,
            durable: 4,
        },
        snapshot: recorded(),
    };
    assert_eq!(
        encode(Ok(Output::plain(attach)), None),
        json!({"live":{"epoch":1,"generation":2,"seq":3,"durable":4},"agents":[],"z":null}),
    );
    let page = live::Page {
        cursor: None,
        items: vec![],
        resync: true,
        durable_changed: false,
    };
    assert_eq!(
        encode(Ok(Output::plain(page)), None).to_string(),
        r#"{"cursor":null,"durable_changed":false,"items":[],"resync":true}"#,
    );
    let probe = Report::new(Outcome::NotConfigured, None, "codex_not_configured");
    assert_eq!(
        encode(Ok(Output::plain(probe)), None).to_string(),
        r#"{"code":"codex_not_configured","executable":null,"outcome":"not_configured"}"#,
    );
}

#[test]
fn typed_usage_keeps_float_and_large_counter_bytes_from_the_previous_projection() {
    use crate::runtime::domain::{ModelCounters, ModelUsage, NativeSessionUsage, NativeUsage};
    let report = provider::usage::Report {
        auth: "login".into(),
        provider: provider::usage::Provider::Claude,
        generation: "generation".into(),
        observed_at: 1,
        windows: vec![provider::usage::WindowReport {
            name: "window".into(),
            used_percent: -0.0,
            resets_at: None,
            minutes: Some(300),
        }],
        credits: None,
        extra_usage: None,
        native_session: Some(NativeSessionUsage {
            agent_id: "agent".into(),
            usage: NativeUsage {
                session_id: "session".into(),
                turn_id: "turn".into(),
                observed_at: 1,
                outcome: crate::runtime::domain::ProviderTurnStatus::Completed,
                reset: Some(false),
                models: ModelUsage(std::collections::BTreeMap::from([(
                    "model".into(),
                    ModelCounters {
                        input_tokens: u64::MAX,
                        output_tokens: 0,
                        cache_read_input_tokens: 1,
                        cache_creation_input_tokens: 2,
                        web_search_requests: None,
                    },
                )])),
            },
        }),
    };
    let reports = vec![report];
    let previous = api::value(api::UsageReply {
        usage: api::value(&reports),
    })
    .to_string();
    let actual = encode(Ok(Output::plain(api::UsageReply { usage: reports })), None).to_string();
    assert_eq!(actual, previous);
    assert!(actual.contains("-0.0"));
    assert!(actual.contains("18446744073709551615"));
}
