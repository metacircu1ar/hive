//! Claude relaunch regressions.
use super::*;

#[test]
fn claude_model_change_relaunches_once_and_resumes_the_same_conversation() {
    let mut f = super::super::claude::fixture();
    f.start();
    let id = super::super::claude::spawn(&f, "hold");
    let original = f.wait(&id, |a| a.state == "running");
    change(&f, &id, "haiku", "low");
    let pending = f.status(&id);
    assert_eq!(pending.turn_selection.as_ref().unwrap().model, "sonnet");
    f.ok(json!({"op":"send","target":id,"body":"after-switch"}));
    f.ok(json!({"op":"interrupt","target":id}));
    super::super::claude::answered(&f, &id, "answer:after-switch");
    let updated = f.wait(&id, |a| {
        a.state == "idle" && a.execution_id != original.execution_id
    });
    assert_eq!(updated.thread_id, original.thread_id);
    assert_eq!(updated.session_selection.as_ref().unwrap().model, "haiku");
    assert_eq!(updated.session_selection.as_ref().unwrap().effort, "low");
    let launches: Vec<Value> =
        fs::read_to_string(super::super::claude::home(&f).join("mock-launches.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert_eq!(launches[1]["resume"], true);
    assert_eq!(launches[1]["model"], "claude-haiku-4-5");
    assert!(f.reports().iter().all(|r| r["confirmed"] == true));
    f.shutdown();
}

#[test]
fn model_change_during_claude_relaunch_applies_after_the_reserved_turn() {
    let mut f = super::super::claude::fixture();
    f.start();
    let id = super::super::claude::spawn(&f, "hold");
    let initial = f.wait(&id, |a| a.state == "running");
    change(&f, &id, "haiku", "low");
    let home = super::super::claude::home(&f);
    let release = f.tmp.path().join("settings-release");
    let _release_on_failure = support::ReleaseFile::new(&release);
    f.update_scenario(Provider::Claude, |value| {
        value["settings_gate"] = json!(release);
    });
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    f.ok(json!({"op":"interrupt","target":id}));
    wait_until(
        SETTINGS_TIMEOUT,
        PROVIDER_POLL_INTERVAL,
        "Claude settings gate",
        || home.join("settings-waiting").exists().then_some(()),
    );
    change(&f, &id, "sonnet", "medium");
    fs::write(release, "go").unwrap();
    let reserved = f.wait(&id, |a| {
        a.state == "running" && a.execution_id != initial.execution_id
    });
    assert_eq!(reserved.model, "sonnet");
    assert_eq!(reserved.options_revision, 2);
    assert_eq!(reserved.turn_selection.as_ref().unwrap().model, "haiku");
    assert_eq!(reserved.turn_selection.as_ref().unwrap().revision, 1);
    f.ok(json!({"op":"send","target":id,"body":"after-two-changes"}));
    f.ok(json!({"op":"interrupt","target":id}));
    super::super::claude::answered(&f, &id, "answer:after-two-changes");
    let final_state = f.wait(&id, |a| a.state == "idle");
    assert_eq!(
        final_state.session_selection.as_ref().unwrap().model,
        "sonnet"
    );
    assert_eq!(final_state.session_selection.as_ref().unwrap().revision, 2);
    assert_eq!(final_state.thread_id, initial.thread_id);
    let launches = fs::read_to_string(home.join("mock-launches.jsonl")).unwrap();
    assert_eq!(launches.lines().count(), 3);
    f.shutdown();
}

#[test]
fn claude_session_options_relaunch_and_reset_without_stale_settings() {
    let mut f = Harness::builder(Provider::Claude)
        .scenario(json!({"provider_version":"2.1.287"}))
        .build();
    f.start();
    let id = super::super::claude::spawn(&f, "hold");
    let first = f.wait(&id, |a| a.state == "running");
    f.ok(json!({"op":"set_options","target":id,"options":{
        "claude.auto_compact_window":200_000,
        "claude.max_output_tokens":50000,
        "claude.max_mcp_output_tokens":4000
    }}));
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    f.ok(json!({"op":"interrupt","target":id}));
    let second = f.wait(&id, |a| {
        a.state == "running" && a.execution_id != first.execution_id
    });
    f.ok(json!({"op":"set_options","target":id,"options":{
        "claude.auto_compact_window":null,
        "claude.max_output_tokens":null,
        "claude.max_mcp_output_tokens":null
    }}));
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    f.ok(json!({"op":"interrupt","target":id}));
    f.wait(&id, |a| {
        a.state == "running" && a.execution_id != second.execution_id
    });

    let launches: Vec<Value> =
        fs::read_to_string(super::super::claude::home(&f).join("mock-launches.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(launches.len(), 3, "{launches:?}");
    assert_eq!(launches[1]["settings"]["autoCompactWindow"], 200_000);
    assert_eq!(
        launches[1]["environment"]["CLAUDE_CODE_MAX_OUTPUT_TOKENS"],
        "50000"
    );
    assert_eq!(launches[1]["environment"]["MAX_MCP_OUTPUT_TOKENS"], "4000");
    assert!(launches[2]["settings"].get("autoCompactWindow").is_none());
    assert_eq!(
        launches[2]["environment"]["CLAUDE_CODE_MAX_OUTPUT_TOKENS"],
        Value::Null
    );
    assert_eq!(
        launches[2]["environment"]["MAX_MCP_OUTPUT_TOKENS"],
        Value::Null
    );
    f.shutdown();
}
