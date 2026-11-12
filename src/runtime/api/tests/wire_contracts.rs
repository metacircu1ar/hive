//! Wire contracts regressions.
use super::*;

#[test]
fn tool_schemas_match_current_contract() {
    if std::env::var_os("HIVE_UPDATE_TOOL_SCHEMAS").is_some() {
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/src/runtime/api/").to_owned()
                + if cfg!(feature = "comb") {
                    "tool_schemas.json"
                } else {
                    "tool_schemas_core.json"
                },
            format!("{}\n", serde_json::to_string_pretty(tools()).unwrap()),
        )
        .unwrap();
        return;
    }
    let baseline: Value = serde_json::from_str(if cfg!(feature = "comb") {
        include_str!("../tool_schemas.json")
    } else {
        include_str!("../tool_schemas_core.json")
    })
    .unwrap();
    assert_eq!(tools(), &baseline);
}

#[test]
fn cli_request_serialization_preserves_nulls_omissions_and_batch_values() {
    let request = Request::Spawn(Box::new(SpawnArgs {
        role: "test".to_owned().into(),
        body: "body".to_owned().into(),
        cwd: PathBuf::from("/tmp").into(),
        auth: Input::nullable(None),
        model: Input::nullable(None),
        effort: Input::nullable(None),
        lifetime: Input::Missing,
        ..SpawnArgs::default()
    }));
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        json!({
            "op": "spawn",
            "role": "test",
            "body": "body",
            "cwd": "/tmp",
            "auth": null,
            "model": null,
            "effort": null
        })
    );
    assert_eq!(
        serde_json::to_value(Request::Agents(AgentsArgs {
            after: 0.into(),
            limit: 200.into(),
            parent: Input::nullable(None),
            auth: Input::Missing,
            ..AgentsArgs::default()
        }))
        .unwrap(),
        json!({"op":"agents","after":0,"limit":200,"parent":null})
    );
    assert_eq!(
        serde_json::to_value(Request::Messages(MessagesArgs {
            target: "a".to_owned().into(),
            before: Input::nullable(None),
            limit: 100.into()
        }))
        .unwrap(),
        json!({"op":"messages","target":"a","before":null,"limit":100})
    );
    assert_eq!(
        serde_json::to_value(Request::Status(StatusArgs {
            target: "a".to_owned().into(),
            ..Default::default()
        }))
        .unwrap(),
        json!({"op":"status","target":"a"})
    );
    assert_eq!(
        serde_json::to_value(Request::Status(StatusArgs {
            target: "a".to_owned().into(),
            instructions: false.into()
        }))
        .unwrap(),
        json!({"op":"status","target":"a","instructions":false})
    );
    for operations in [
        json!(false),
        json!([{"op":"send","request_id":"x","extra":3.0}, null]),
    ] {
        let request = Request::Batch(BatchArgs {
            join: Input::Missing,
            operations: Input::from_value(operations.clone()),
        });
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({"op":"batch","operations":operations})
        );
    }
}

#[test]
fn tool_validation_accepts_current_per_operation_inputs() {
    for (name, args) in [
        (
            "hive_spawn",
            json!({"body":"task","request_id":"empty-instructions","instructions":""}),
        ),
        ("hive_agents", json!({"limit":0})),
        ("hive_read", json!({"target":"a","limit":0})),
        ("hive_messages", json!({"target":"a","limit":0,"before":0})),
        (
            "hive_spawn",
            json!({"role":" ","body":"body","request_id":"x"}),
        ),
        (
            "hive_batch",
            json!({
                "request_id": "outer",
                "operations": [
                    {
                        "op": "terminate",
                        "target": "a",
                        "request_id": "inner"
                    }
                ]
            }),
        ),
    ] {
        validate_tool(name, &args).unwrap();
    }
}
