//! Tool callbacks regressions.
use super::*;

#[test]
#[cfg(feature = "comb")]
fn comb_and_extra_mcp_names_obey_both_permission_paths_and_canary() {
    for (tool, allowed) in [
        ("mcp__hive__comb_get", true),
        ("mcp__hive__comb_graph_query", true),
        ("mcp__hive__comb_promote", false),
        ("mcp__hive__hive_send", true),
        ("mcp__hive__hive_invented", false),
        ("mcp__docs__lookup", true),
        ("mcp__docs_extra__lookup", false),
        ("mcp__unconfigured__lookup", false),
    ] {
        let mut f = Fixture::with_extensions(None, Uuid::new_v4().to_string(), |config| {
            config.comb_reviewer = Some(false);
            config.mcp_servers.insert(
                "docs".into(),
                crate::config::McpServer {
                    command: "fixture".into(),
                    args: vec![],
                    env: std::collections::BTreeMap::default(),
                },
            );
        });
        f.turn();
        f.assistant(
            None,
            "claude-sonnet-5",
            json!([{ "type":"tool_use", "id":"tool-1", "name":tool, "input":{} }]),
        );
        assert_eq!(
            f.hook(tool, json!({}))["permissionDecision"],
            if allowed { "allow" } else { "deny" },
            "{tool}"
        );
        let reply = f.codec.receive(json!({"type":"control_request","request_id":"permission","request":{"subtype":"can_use_tool","tool_name":tool,"tool_use_id":"tool-1","input":{}}})).unwrap();
        assert_eq!(
            reply[0]["response"]["response"]["behavior"],
            if allowed { "allow" } else { "deny" },
            "{tool}"
        );
    }
    let mut f = Fixture::with_extensions(
        Some(SteeringPermit::Canary),
        Uuid::new_v4().to_string(),
        |config| config.comb_reviewer = Some(true),
    );
    f.turn();
    f.assistant(
        None,
        "claude-sonnet-5",
        json!([{ "type":"tool_use", "id":"tool-1", "name":"mcp__hive__comb_get", "input":{} }]),
    );
    assert_eq!(
        f.hook("mcp__hive__comb_get", json!({}))["permissionDecision"],
        "deny"
    );
}

#[test]
fn mcp_error_and_cancellation_settle_distinct_request_ids() {
    let mut f = Fixture::new();
    f.turn();
    let frame = json!({"type":"control_request","request_id":"outer","request":{"subtype":"mcp_message","server_name":"hive","message":{"jsonrpc":"2.0","id":"inner","method":"tools/call","params":{"name":"hive_agents","arguments":{}}}}});
    f.codec.receive(frame.clone()).unwrap();
    let id = f.codec.pop().unwrap()["id"].clone();
    let error = f
        .codec
        .send(outgoing::unsupported(&id, "Test tool failure."))
        .unwrap();
    assert_eq!(error[0]["response"]["request_id"], "outer");
    assert_eq!(
        error[0]["response"]["response"]["mcp_response"]["id"],
        "inner"
    );
    assert!(error[0]["response"]["response"]["mcp_response"]["error"].is_object());
    f.codec.receive(frame).unwrap();
    let id = f.codec.pop().unwrap()["id"].clone();
    f.codec
        .receive(json!({"type":"control_cancel_request","request_id":"outer"}))
        .unwrap();
    assert!(
        f.codec
            .send(outgoing::unsupported(&id, "Test tool failure."))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn tools_list_keeps_the_public_schema_and_request_identity() {
    let mut fixture = Fixture::new();
    let expected = super::envelopes::expected_catalog();
    for id in [json!("opaque"), json!(17), json!(null)] {
        let replies = fixture
            .codec
            .receive(json!({
                "type":"control_request", "request_id":"tools", "request":{
                    "subtype":"mcp_message", "server_name":"hive", "message":{
                        "jsonrpc":"2.0", "id":id, "method":"tools/list", "params":{}
                    }
                }
            }))
            .unwrap();
        let response = &replies[0]["response"]["response"]["mcp_response"];
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["tools"], expected);
        assert_eq!(replies[0]["response"]["request_id"], "tools");
    }
}

#[test]
#[cfg(feature = "comb")]
fn abandoned_comb_callbacks_do_not_accumulate_cancelled_tombstones() {
    let mut f = Fixture::new();
    f.turn();
    let mut ids = Vec::new();
    for n in 0..300 {
        let native = if n < 2 {
            "retransmission".into()
        } else {
            format!("native-{n}")
        };
        f.codec.receive(json!({"type":"control_request","request_id":native,"request":{
            "subtype":"mcp_message","server_name":"hive","message":{"jsonrpc":"2.0","id":"same-inner-id","method":"tools/call","params":{"name":"comb_get","arguments":{"slug":"concepts/test"}}}
        }})).unwrap();
        let frame = f.codec.pop().unwrap();
        ids.push(frame["params"]["callId"].clone());
        let replies = f.codec.receive(json!({"type":"control_request","request_id":"cancel","request":{
            "subtype":"mcp_message","server_name":"hive","message":{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"same-inner-id"}}
        }})).unwrap();
        for reply in replies {
            f.codec.receive(reply).unwrap();
        }
        f.codec.forget_tool(&frame["id"]).unwrap();
    }
    assert_eq!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);
}

#[test]
#[cfg(feature = "comb")]
fn interrupt_retires_comb_callbacks_not_yet_delivered_to_the_worker() {
    let mut f = Fixture::new();
    let turn = f.turn();
    // Decode two callbacks before the worker handles even the first frame.
    for n in 0..2 {
        f.codec.receive(json!({"type":"control_request","request_id":format!("comb-{n}"),"request":{
            "subtype":"mcp_message","server_name":"hive","message":{"jsonrpc":"2.0","id":n,"method":"tools/call","params":{"name":"comb_get","arguments":{"slug":"concepts/test"}}}
        }})).unwrap();
    }
    let admitted = f.codec.pop().unwrap();
    f.codec.forget_comb_turn(&turn);
    f.codec.forget_tool(&admitted["id"]).unwrap();
    let interrupt = f
        .codec
        .send(outgoing::request(999, outgoing::interrupt(&f.id, &turn)))
        .unwrap();
    f.codec.receive(json!({"type":"control_response","response":{"subtype":"success","request_id":interrupt[0]["request_id"],"response":{"still_queued":[],"cancelled":[]}}})).unwrap();
    f.codec.receive(json!({"type":"result","session_id":f.id,"subtype":"success","is_error":false,"terminal_reason":"aborted_tools","user_message_uuid":turn})).unwrap();
    let queued = f.codec.pop().unwrap();
    assert_eq!(queued["method"], "item/tool/call");
    f.codec.forget_tool(&queued["id"]).unwrap(); // No unknown-ID reply.
    assert_eq!(f.codec.pop().unwrap()["id"], 999);
    assert_eq!(
        f.codec.pop().unwrap()["params"]["turn"]["status"],
        "interrupted"
    );
    assert_ne!(f.turn(), turn);
}
