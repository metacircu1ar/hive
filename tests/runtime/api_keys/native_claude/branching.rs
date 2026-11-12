//! Actual native fork/resume and counter observations after private work.
use super::*;

#[test]
#[ignore = "requires HIVE_NATIVE_CLAUDE; real CLI, isolated scripted HTTP, no account/model"]
fn fork_and_resume_preserve_main_history_after_native_children() {
    let mut f = NativeFixture::new("branch");
    f.hive.start();
    let source = f.hive.ok(
        json!({"op":"spawn","role":"native","cwd":f.hive.tmp.path(),"body":"NATIVE_ROOT_PARALLEL"}),
    );
    let source_id = source["agent_id"].as_str().unwrap();
    super::super::super::claude::answered(&f.hive, source_id, "NATIVE_ROOT_DONE");
    f.assert_parent_output(source_id, "NATIVE_ROOT_DONE");
    let parent = f.hive.settled(source_id);
    f.record_usage(&parent);
    let child_cwd = f.hive.tmp.path().join("branch-workspace");
    fs::create_dir(&child_cwd).unwrap();
    let spawned = f.hive.ok(
        json!({"op":"spawn","from_agent":source_id,"cwd":child_cwd,"body":"NATIVE_ROOT_BRANCH"}),
    );
    let id = spawned["agent_id"].as_str().unwrap();
    super::super::super::claude::answered(&f.hive, id, "NATIVE_BRANCH_DONE");
    f.assert_parent_output(id, "NATIVE_BRANCH_DONE");
    let branched = f.hive.settled(id);
    assert!(branched.error.is_none(), "{branched:?}");
    assert_eq!(branched.branch_phase.as_deref(), Some("bound"));
    assert_ne!(branched.thread_id, parent.thread_id);
    assert_eq!(branched.native_usage.as_ref().unwrap()["reset"], false);
    f.record_usage(&branched);
    f.hive.ok(json!({"op":"stop","target":id}));
    f.hive.wait(id, |agent| agent.state == "stopped");
    f.hive
        .ok(json!({"op":"send","target":id,"body":"NATIVE_ROOT_RESUME"}));
    f.hive.ok(json!({"op":"resume","target":id}));
    super::super::super::claude::answered(&f.hive, id, "NATIVE_RESUME_DONE");
    f.assert_parent_output(id, "NATIVE_RESUME_DONE");
    let resumed = f.hive.settled(id);
    assert_eq!(resumed.thread_id, branched.thread_id);
    f.record_usage(&resumed);
    let requests = NativeFixture::rows(&f.receipts);
    for role in ["NATIVE_ROOT_BRANCH", "NATIVE_ROOT_RESUME"] {
        assert!(
            requests
                .iter()
                .any(|row| row["role"] == role && row["inherited_root"] == true),
            "{requests:?}"
        );
    }
    let tasks = NativeFixture::rows(&f.trace)
        .iter()
        .filter(|row| row["subtype"] == "task_started")
        .count();
    assert_eq!(
        tasks, 2,
        "resuming/forking must not rerun the old native children"
    );
    f.hive.ok(json!({"op":"terminate","target":id}));
    f.hive.ok(json!({"op":"terminate","target":source_id}));
    f.hive.shutdown();
    let source_cwd = f.hive.tmp.path().to_owned();
    f.record_history(
        &source_cwd,
        &source_cwd,
        parent.thread_id.as_deref().unwrap(),
    );
    f.record_history(
        &child_cwd,
        &source_cwd,
        resumed.thread_id.as_deref().unwrap(),
    );
    f.assert_capture();
    f.export().unwrap();
}
