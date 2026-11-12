//! The permission gate precedes SDK dispatch; identical parent calls still work.
use super::*;

#[test]
fn child_hosted_denial_creates_no_callback_and_the_parent_receives_its_own_reply() {
    let mut f = Native::new(Sandbox::ReadOnly);
    let turn = f.0.turn();
    f.open("owner");
    f.task("child", "owner").unwrap();
    let name = "mcp__hive__hive_agents";
    f.announce(Some("owner"), "child-sdk", name, json!({}));
    assert_eq!(
        decision(&f.pre(Some("child"), "child-sdk", name, json!({}))),
        "deny"
    );
    assert!(
        !f.drain()
            .iter()
            .any(|frame| frame["method"] == "item/tool/call")
    );
    f.announce(None, "parent-sdk", name, json!({}));
    assert_eq!(
        decision(&f.pre(None, "parent-sdk", name, json!({}))),
        "allow"
    );
    f.receive(json!({"type":"control_request", "request_id":"parent-rpc", "request":{
        "subtype":"mcp_message", "server_name":"hive", "message":{"jsonrpc":"2.0", "id":"same-arguments",
            "method":"tools/call", "params":{"name":"hive_agents", "arguments":{}}}}})).unwrap();
    let frames = f.drain();
    let calls: Vec<_> = frames
        .iter()
        .filter(|frame| frame["method"] == "item/tool/call")
        .collect();
    assert_eq!(calls.len(), 1);
    let replies =
        f.0.codec
            .send(outgoing::tool_reply(
                &calls[0]["id"],
                &json!({"agents":[]}),
                true,
            ))
            .unwrap();
    assert_eq!(replies[0]["response"]["request_id"], "parent-rpc");
    assert_eq!(
        replies[0]["response"]["response"]["mcp_response"]["id"],
        "same-arguments"
    );
    assert_eq!(
        replies[0]["response"]["response"]["mcp_response"]["result"]["isError"],
        false
    );
    f.receive(replies[0].clone()).unwrap();
    f.post(None, "parent-sdk", name);
    f.post(None, "owner", "Agent");
    f.finish(&turn).unwrap();
    assert!(f.0.codec.quiescent());
}
