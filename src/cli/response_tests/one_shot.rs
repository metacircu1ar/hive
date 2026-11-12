//! Spawn waits use the accepted identity and the same task observer as wait.
use super::*;
use clap::Parser;

fn spawn_wait(stub: &Stub) -> super::super::CommandOutcome {
    let cli = super::super::Cli::try_parse_from([
        "hivectl",
        "spawn",
        "--cwd",
        "/tmp",
        "--body",
        "review",
        "--one-shot",
        "--wait",
        "--timeout",
        "1",
    ])
    .unwrap();
    let super::super::Command::Spawn(args) = cli.command else {
        panic!("spawn command")
    };
    args.run(stub.root(), "spawn-retry", args.request().unwrap())
        .unwrap()
}

#[test]
fn one_shot_spawn_wait_uses_accepted_identity_and_returns_selected_result() {
    for (selection, text, target) in [
        (Some("final_answer"), Some("review complete"), None),
        (
            Some("last_assistant_message"),
            Some("preview"),
            Some("artifact:full"),
        ),
        (Some("no_text"), Some(""), None),
        (None, Some("explicit result"), None),
    ] {
        let mut agent = status_agent("completed", "stopped", false);
        agent["result"] = json!(text);
        agent["result_selection"] = json!(selection);
        agent["result_read_target"] = json!(target);
        let stub = Stub::new(vec![
            json!({"status":"accepted","agent_id":"agent-id","message_id":"startup"}),
            json!({"agent":agent,"attachment_store_bytes":0}),
        ]);
        let result = spawn_wait(&stub);
        assert!(result.success);
        assert_eq!(
            result.value,
            json!({"agent_id":"agent-id","result":text,
            "result_selection":selection,"result_read_target":target})
        );
        let requests = stub.finish();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["id"], "spawn-retry");
        assert_eq!(requests[0]["args"]["one_shot"], true);
        assert!(requests[0]["args"].get("wait").is_none());
        assert!(requests[0]["args"].get("timeout").is_none());
        assert_eq!(
            requests[1]["args"],
            json!({"op":"status","target":"agent-id"})
        );
    }
}

#[test]
fn one_shot_spawn_wait_follows_server_auth_recovery_without_mutating_the_task() {
    for continuation in ["waiting", "runnable", "none"] {
        let mut failed = status_agent("open", "failed", false);
        failed["automatic_continuation"] = json!(continuation);
        let mut replies = vec![
            json!({"status":"accepted","agent_id":"agent-id","message_id":"startup"}),
            json!({"agent":failed,"attachment_store_bytes":0}),
        ];
        if continuation != "none" {
            replies.push(json!({"agent":status_agent("completed", "stopped", false),
                "attachment_store_bytes":0}));
        }
        let stub = Stub::new(replies);
        let result = spawn_wait(&stub);
        assert_eq!(result.success, continuation != "none");
        if !result.success {
            assert_eq!(result.value["agent"], failed);
        }
        let requests = stub.finish();
        assert!(
            requests[1..]
                .iter()
                .all(|request| request["args"]["op"] == "status")
        );
    }
}

#[test]
fn one_shot_spawn_acknowledgement_is_strict_before_observing() {
    use crate::runtime::api::DaemonReply;
    for response in [
        json!({}),
        json!(["accepted", "a", "m"]),
        json!({"status":"other","agent_id":"a","message_id":"m"}),
        json!({"status":"accepted","agent_id":null,"message_id":"m"}),
        json!({"status":"accepted","agent_id":"a"}),
        json!({"status":"accepted","agent_id":"a","message_id":"m","extra":true}),
    ] {
        assert_eq!(
            DaemonReply::new(&response)
                .and_then(|reply| reply.spawn())
                .err()
                .unwrap()
                .code(),
            "daemon_protocol_error"
        );
    }
}
