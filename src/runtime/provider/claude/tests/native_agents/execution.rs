//! Real owned shell evidence under synthetic foreground native-task frames.
use super::*;

#[test]
fn interrupt_revokes_unused_child_shell_permits() {
    let mut f = Native::new(Sandbox::WorkspaceWrite);
    let turn = f.0.turn();
    f.open("owner");
    f.task("child", "owner").unwrap();
    let root_input = json!({"file_path":f.0.dir.path().canonicalize().unwrap().join("late.txt")});
    f.announce(None, "late-write", "Write", root_input.clone());
    // No assistant tool frame: the explicit registered child identity owns it.
    let replies = f.pre(
        Some("child"),
        "bash",
        "Bash",
        json!({"command":"echo private"}),
    );
    assert_eq!(decision(&replies), "allow");
    let input = replies[0]["response"]["response"]["hookSpecificOutput"]["updatedInput"].clone();
    assert!(!f.0.codec.guard.clean());
    f.0.codec
        .send(outgoing::request(100, outgoing::interrupt(&f.0.id, &turn)))
        .unwrap();
    assert!(f.0.codec.guard.clean());
    assert_eq!(
        decision(&f.pre(None, "late-write", "Write", root_input)),
        "deny"
    );
    let late = f.receive(json!({"type":"control_request", "request_id":"late", "request":{
        "subtype":"can_use_tool", "tool_name":"Bash", "tool_use_id":"bash", "agent_id":"child", "input":input}})).unwrap();
    assert_eq!(late[0]["response"]["response"]["behavior"], "deny");
    let sdk = f
        .receive(
            json!({"type":"control_request", "request_id":"late-sdk", "request":{
        "subtype":"mcp_message", "server_name":"hive", "message":{"jsonrpc":"2.0", "id":1,
            "method":"tools/call", "params":{"name":"hive_agents", "arguments":{}}}}}),
        )
        .unwrap();
    assert!(sdk[0]["response"]["response"]["mcp_response"]["error"].is_object());
    assert!(
        !f.drain()
            .iter()
            .any(|frame| frame["method"] == "item/tool/call")
    );
}

#[test]
fn interrupt_cannot_complete_until_the_registered_child_shell_is_physically_gone() {
    let mut f = Native::new(Sandbox::WorkspaceWrite);
    let turn = f.0.turn();
    f.open("owner");
    f.task("child", "owner").unwrap();
    let replies = f.pre(
        Some("child"),
        "bash",
        "Bash",
        json!({"command":
        "touch child-ready; while ! test -e child-release; do sleep 0.01; done"}),
    );
    let command =
        replies[0]["response"]["response"]["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .unwrap();
    let dir = f.0.codec.guard.directory();
    f.0.codec.guard.bind(std::process::id());
    let mut child = crate::test_support::ChildGuard(
        std::process::Command::new(dir.join("hive-bash"))
            .args(["-c", command])
            .env(crate::shell::SOCKET_ENV, dir.join("socket"))
            .current_dir(f.0.dir.path())
            .spawn()
            .unwrap(),
    );
    crate::test_support::wait_until(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(10),
        || {
            f.0.codec.guard.service().unwrap();
            f.0.dir.path().join("child-ready").exists().then_some(())
        },
    );
    let interrupt =
        f.0.codec
            .send(outgoing::request(100, outgoing::interrupt(&f.0.id, &turn)))
            .unwrap();
    assert!(
        !f.0.codec.guard.clean(),
        "registered evidence survived owner retirement"
    );
    f.receive(json!({"type":"control_response", "response":{"subtype":"success", "request_id":interrupt[0]["request_id"],
        "response":{"still_queued":[], "cancelled":[]}}})).unwrap();
    f.receive(
        json!({"type":"result", "session_id":f.0.id, "subtype":"success", "is_error":false,
        "terminal_reason":"aborted_tools", "user_message_uuid":turn}),
    )
    .unwrap();
    assert!(!f.drain().iter().any(|f| f["method"] == "turn/completed"));
    std::fs::write(f.0.dir.path().join("child-release"), b"release").unwrap();
    crate::test_support::wait_until(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(10),
        || {
            f.0.codec.guard.service().unwrap();
            (f.0.codec.guard.clean() && child.0.try_wait().unwrap().is_some()).then_some(())
        },
    );
    assert!(child.0.wait().unwrap().success());
    f.0.codec.settle();
    assert_eq!(
        f.drain().last().unwrap()["params"]["turn"]["status"],
        "interrupted"
    );
    assert!(f.0.codec.quiescent());
}
