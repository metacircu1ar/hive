//! One-shot behavior through the installed control/provider paths, using mock agents.
use super::*;
mod cli_wait;
mod end_to_end;

provider_matrix!(
    one_shot_completion_releases_sessions_after_answering_approval,
    approval_case
);

fn approval_case(provider: Provider) {
    let mut f = Harness::builder(provider).build();
    f.start();
    let role = if provider == Provider::Claude {
        "claude"
    } else {
        "implementor"
    };
    let response =
        f.batch::<harness::views::SpawnView>(json!({"op":"batch","join":{},"operations":[{
            "op":"spawn","role":role,"cwd":f.tmp.path(),"one_shot":true,"request_id":"worker",
            "body":"ask:{\"message\":\"Need an answer\",\"request_id\":\"question\"}"
        }]}));
    let join = response.join.as_ref().unwrap().join_id.as_str();
    let snapshot = f.join_snapshot(json!({"op":"read","target":format!("join:{join}")}));
    let id = snapshot.members[0].target.as_str();
    let waiting = f.wait(id, |a| {
        a.state == "idle" && a.waiting_on.as_deref() == Some("human")
    });
    assert_eq!(waiting.task_outcome, "open");
    assert_eq!(waiting.one_shot_waiting, "question");
    assert_eq!(
        f.join_snapshot(json!({"op":"read","target":format!("join:{join}")}))
            .members[0]
            .outcome,
        "pending"
    );
    let answered =
        f.ok(json!({"op":"respond","approval_id":waiting.approval_id,"action":"accept"}));
    let completed = f.wait(id, |a| {
        a.task_outcome == "completed" && a.state == "stopped"
    });
    assert_eq!(completed.thread_id, waiting.thread_id);
    assert_eq!(completed.completed_by.as_deref(), Some("one_shot"));
    assert_eq!(
        completed.result_selection.as_deref(),
        Some(if provider == Provider::Claude {
            "last_assistant_message"
        } else {
            "final_answer"
        })
    );
    assert!(completed.conversation_clean);
    let result = f.join_snapshot(json!({"op":"read","target":format!("join:{join}")}));
    assert_eq!(result.state, "completed");
    assert_eq!(result.members[0].outcome, "completed");
    assert!(result.members[0].task_completed.is_none());
    assert_eq!(
        result.members[0]
            .output
            .as_ref()
            .map(|output| output.text.as_str()),
        completed.result.as_deref()
    );
    assert_eq!(
        f.message_result(json!({"op":"message_result","message_id":answered["message_id"]}))
            .outcome,
        "replied"
    );
    let events = f.read(id);
    let entries = events.events;
    assert_eq!(
        entries
            .iter()
            .filter(|e| e.kind == "task_completed")
            .count(),
        1
    );
    assert!(entries.iter().any(|e| matches!(
        e.kind.as_str(),
        "provider_shutdown" | "provider_session_released"
    ) && e.data["confirmed"] == true));
    f.shutdown();
}

#[test]
fn one_shot_redirect_consumes_guidance_before_completing_on_the_same_thread() {
    let mut f = Harness::new();
    f.start();
    let response = f.spawned(
        json!({"op":"spawn","role":"implementor","cwd":f.tmp.path(),"body":"hold","one_shot":true}),
    );
    let id = response.agent_id.as_str();
    let running = f.wait(id, |a| a.state == "running");
    f.ok(json!({"op":"send","target":id,"body":"reply:redirect result","mode":"redirect"}));
    let completed = f.wait(id, |a| {
        a.task_outcome == "completed" && a.state == "stopped"
    });
    assert_eq!(completed.thread_id, running.thread_id);
    assert!(
        completed
            .result
            .as_deref()
            .unwrap()
            .ends_with("reply:redirect result")
    );
    let messages = f.mailbox(id);
    let messages = messages.messages;
    assert_eq!(
        messages.iter().filter(|m| m.state == "interrupted").count(),
        1
    );
    assert_eq!(
        messages.iter().filter(|m| m.state == "completed").count(),
        1
    );
    f.shutdown();
}

#[test]
fn one_shot_branches_do_not_inherit_the_source_flag() {
    let mut f = Harness::new();
    f.start();
    let reply = f.spawned(json!({"op":"spawn","role":"implementor","cwd":f.tmp.path(),
        "body":"ask:{\"message\":\"Keep this task open\",\"request_id\":\"source-question\"}",
        "one_shot":true}));
    let source = reply.agent_id.as_str();
    f.wait(source, |a| {
        a.state == "idle" && a.waiting_on.as_deref() == Some("human")
    });
    for selection in [None, Some(false), Some(true)] {
        let mut request = json!({"op":"spawn","from_agent":source,"body":"reply: branch answer"});
        if let Some(selection) = selection {
            request["one_shot"] = json!(selection);
        }
        let response = f.ok(request);
        let id = response["agent_id"].as_str().unwrap();
        let child = f.wait(id, |a| a.branch_phase.as_deref() == Some("bound"));
        assert_eq!(child.branched_from_agent_id.as_deref(), Some(source));
        assert_eq!(child.one_shot, selection == Some(true));
    }
    assert!(f.status(source).one_shot);
    f.shutdown();
}
