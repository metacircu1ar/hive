//! One-shot CLI results and hosted parent wakeups through both mock providers.
use super::{cli_wait::cli, *};

fn role(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "implementor",
        Provider::Claude => "claude",
    }
}

provider_matrix!(one_shot_spawn_wait_returns_the_completed_result, spawn_case);

fn spawn_case(provider: Provider) {
    let mut f = Harness::builder(provider).build();
    f.start();
    let (result, output) = cli(
        &f,
        &[
            "spawn",
            role(provider),
            "--cwd",
            f.tmp.path().to_str().unwrap(),
            "--body",
            "reply:review complete",
            "--one-shot",
            "--wait",
            "--timeout",
            "15",
            "--request-id",
            "successful-one-shot",
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{result}");
    let id = result["agent_id"].as_str().unwrap();
    let status = f.wait(id, |a| {
        a.task_outcome == "completed" && a.state == "stopped"
    });
    assert_eq!(
        result,
        json!({
            "agent_id": id, "result": status.result,
            "result_selection": status.result_selection,
            "result_read_target": status.result_read_target
        })
    );
    assert!(
        result["result"]
            .as_str()
            .unwrap()
            .ends_with("reply:review complete")
    );
    assert_eq!(status.completed_by.as_deref(), Some("one_shot"));
    assert_eq!(result["result_read_target"], Value::Null);
    let progress = String::from_utf8(output.stderr).unwrap();
    assert!(progress.contains("request ID successful-one-shot"));
    assert!(progress.contains(&format!("agent {id};")));
    f.shutdown();
}

provider_matrix!(one_shot_cli_join_collects_each_reviewer_once, join_case);

fn join_case(provider: Provider) {
    let mut f = Harness::builder(provider).build();
    f.start();
    let operations: Vec<_> = (0..2)
        .map(|i| {
            json!({
                "op":"spawn", "role":role(provider), "cwd":f.tmp.path(), "one_shot":true,
                "body":format!("reply:review {i}"), "request_id":format!("reviewer-{i}")
            })
        })
        .collect();
    let path = f.tmp.path().join("one-shot-round.json");
    support::publish_fixture(&path, &serde_json::to_vec(&operations).unwrap());
    let args = [
        "batch",
        "--operations-file",
        path.to_str().unwrap(),
        "--join",
        "--request-id",
        "one-shot-round",
    ];
    let (accepted, output) = cli(&f, &args);
    assert!(output.status.success(), "{accepted}");
    let join = accepted["join"]["join_id"].as_str().unwrap();
    let wait_args = ["wait", "--join", join, "--timeout", "15"];
    let (done, output) = cli(&f, &wait_args);
    assert!(output.status.success(), "{done}");
    let done: harness::views::joins::JoinSnapshot = support::reply::observe(done);
    assert_eq!(done.state, "completed");
    check_join_members(&f, &done.members);
    // Re-observation and acceptance retry neither rerun nor add a member.
    let (retry, output) = cli(&f, &args);
    assert!(output.status.success(), "{retry}");
    assert_eq!(retry, accepted);
    let (again, output) = cli(&f, &wait_args);
    assert!(output.status.success(), "{again}");
    assert_eq!(again, serde_json::to_value(&done).unwrap());
    assert_eq!(
        f.ok(json!({"op":"agents"}))["agents"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    f.shutdown();
}

fn check_join_members(f: &Harness, members: &[harness::views::joins::JoinMember]) {
    assert_eq!(members.len(), 2);
    assert_ne!(members[0].target, members[1].target);
    for (i, member) in members.iter().enumerate() {
        assert_eq!(member.position, i);
        assert_eq!(member.outcome, "completed");
        let id = member.target.as_str();
        let status = f.wait(id, |a| a.state == "stopped");
        assert_eq!(status.task_outcome, "completed");
        assert_eq!(status.completed_by.as_deref(), Some("one_shot"));
        assert_eq!(
            member.output.as_ref().map(|output| output.text.as_str()),
            status.result.as_deref()
        );
        assert_eq!(member.output_selection, status.result_selection);
        assert!(
            status
                .result
                .as_deref()
                .unwrap()
                .ends_with(&format!("reply:review {i}"))
        );
        assert_eq!(
            f.read(id)
                .events
                .iter()
                .filter(|event| event.kind == "task_completed")
                .count(),
            1
        );
    }
}

provider_matrix!(
    one_shot_child_completion_wakes_its_hosted_parent,
    parent_case
);

fn parent_case(provider: Provider) {
    let mut f = Harness::builder(provider).build();
    f.start();
    let response = f.spawned(json!({"op":"spawn", "role":role(provider),
        "cwd":f.tmp.path(), "body":"ready"}));
    let parent = response.agent_id.as_str();
    let original = f.settled(parent);
    // No join: the hosted batch spawns the child under the parent, so the task
    // result must arrive as a child-completion notice and start another turn.
    let input = json!({"request_id":"delegate-one-shot", "operations":[{
        "op":"spawn", "role":role(provider), "one_shot":true,
        "body":"reply:child review complete", "request_id":"one-shot-child"
    }]});
    f.ok(json!({"op":"send", "target":parent, "body":format!("joined:{input}")}));
    let notice = wait_child_completion(&f, parent);
    let children = f.listed(json!({"op":"agents", "parent":parent}));
    assert_eq!(children.agents.len(), 1);
    let child = children.agents[0].id.as_str();
    let completed = f.wait(child, |a| a.state == "stopped");
    assert_eq!(completed.task_outcome, "completed");
    assert_eq!(completed.completed_by.as_deref(), Some("one_shot"));
    assert_eq!(notice.sender, child);
    assert_eq!(
        notice.body,
        format!(
            "Child task completed: {}",
            completed.result.as_deref().unwrap()
        )
    );
    let parent_status = f.settled(parent);
    assert_eq!(parent_status.thread_id, original.thread_id);
    assert_eq!(parent_status.task_outcome, "open");
    let events = f.read(parent);
    let starts: Vec<_> = events
        .events
        .iter()
        .filter(|event| event.kind == "turn_started")
        .collect();
    assert_eq!(starts.len(), 3);
    assert_eq!(starts[2].data["message_id"], notice.id);
    assert_eq!(
        f.mailbox(parent)
            .messages
            .iter()
            .filter(|message| message.sender == child)
            .count(),
        1
    );
    f.shutdown();
}

fn wait_child_completion(f: &Harness, parent: &str) -> harness::views::MessageView {
    support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        "hosted parent consumed child completion",
        || {
            f.mailbox(parent)
                .messages
                .iter()
                .find(|message| {
                    message.body.as_str().starts_with("Child task completed:")
                        && message.state == "completed"
                })
                .cloned()
        },
    )
}
