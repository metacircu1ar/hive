use super::*;
use crate::config::Provider;

fn references(store: &Store, agent: &str) -> Vec<(String, String, String)> {
    store.sql().cached().query_map("SELECT generation,provider,thread FROM native_sessions WHERE agent=?1 ORDER BY generation,provider,thread", [agent],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap()
}
fn native_child(store: &Store, parent: &Agent, id: &str, thread: &str) -> Agent {
    let mut child = install(store, parent, id);
    child.profile.provider = Provider::Claude;
    child.auth_generation = parent.auth_generation;
    child.thread_id = Some(thread.into());
    child.execution_id = Some(format!("{id}-execution"));
    store.save(&child).unwrap();
    store.settle_branches(state::now()).unwrap();
    child = store.agent(&child.id).unwrap();
    store.begin_branch(&mut child).unwrap();
    child
}

#[test]
fn claude_launch_inherits_only_its_generation_and_rolls_back_before_hold_release() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let mut parent = agent("source", None);
    let source_thread = uuid::Uuid::new_v4().to_string();
    let ancestor = uuid::Uuid::new_v4().to_string();
    let child_thread = uuid::Uuid::new_v4().to_string();
    parent.thread_id = Some(source_thread.clone());
    parent.execution_id = Some("source-execution".into());
    parent.conversation_clean = true;
    parent.state = AgentState::Stopped;
    parent.profile.provider = Provider::Claude;
    store.save(&parent).unwrap();
    store
        .enqueue("prefix", "human", &parent.id, "source context")
        .unwrap();
    finish(&store, &mut parent, "prefix", "turn-prefix");
    store
        .record_claude_cwd(&parent, &source_thread, &parent.cwd)
        .unwrap();
    store
        .record_claude_cwd(&parent, &ancestor, &parent.cwd)
        .unwrap();
    let mut previous = parent.clone();
    previous.auth_generation = uuid::Uuid::new_v4();
    store
        .record_claude_cwd(&previous, &uuid::Uuid::new_v4().to_string(), &parent.cwd)
        .unwrap();
    previous.auth_generation = parent.auth_generation;
    previous.profile.provider = Provider::Codex;
    store
        .record_native_binding(&previous, "unrelated-codex-thread")
        .unwrap();
    let mut child = native_child(&store, &parent, "child", &child_thread);
    let rolled_back: Result<()> = store.atomic(|store| {
        store.record_claude_cwd(&child, &child_thread, &child.cwd)?;
        Err(Error::database_error("Injected rollback."))
    });
    assert!(rolled_back.is_err());
    assert!(references(&store, &child.id).is_empty());
    assert_eq!(store.agent(&parent.id).unwrap().branch_hold, 1);
    store
        .atomic(|store| store.record_claude_cwd(&child, &child_thread, &child.cwd))
        .unwrap();
    let rows = references(&store, &child.id);
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|(generation, provider, _)| generation
        == &parent.auth_generation.to_string()
        && provider == "claude"));
    assert!(rows.iter().any(|(_, _, thread)| thread == &source_thread));
    assert!(rows.iter().any(|(_, _, thread)| thread == &ancestor));
    store
        .atomic(|store| store.bind_branch(&mut child, &child_thread))
        .unwrap();
    assert_eq!(store.agent(&parent.id).unwrap().branch_hold, 0);
    assert_eq!(references(&store, &child.id), rows);
    assert_eq!(
        store
            .query_row_cached(
                "SELECT count(*) FROM native_session_paths WHERE agent='child'",
                [],
                |row| crate::runtime::store::rows::unsigned(row, 0)
            )
            .unwrap(),
        3
    );
    assert_timed_out_branch_keeps_native_references(&store, &parent);
}

fn assert_timed_out_branch_keeps_native_references(store: &Store, parent: &Agent) {
    let thread = uuid::Uuid::new_v4().to_string();
    let mut timed = native_child(store, parent, "timed", &thread);
    store
        .atomic(|store| store.record_claude_cwd(&timed, &thread, &timed.cwd))
        .unwrap();
    let before = references(store, &timed.id);
    store
        .atomic(|store| {
            store.fail_branch(
                &mut timed,
                crate::runtime::branching::BranchFailure::BranchMaterializationTimeout,
                true,
            )
        })
        .unwrap();
    assert_eq!(store.agent(&parent.id).unwrap().branch_hold, 0);
    assert_eq!(references(store, &timed.id), before);
    assert!(store.bind_branch(&mut timed, &thread).is_err());
}

#[test]
fn codex_branch_binding_does_not_inherit_native_session_claims() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let parent = source(&store);
    store
        .record_native_binding(&parent, "source-thread")
        .unwrap();
    let mut child = install(&store, &parent, "child");
    child.execution_id = Some("child-execution".into());
    store.save(&child).unwrap();
    store.settle_branches(state::now()).unwrap();
    child = store.agent(&child.id).unwrap();
    store.begin_branch(&mut child).unwrap();
    store
        .atomic(|store| store.bind_branch(&mut child, "child-thread"))
        .unwrap();
    assert_eq!(references(&store, &child.id).len(), 1);
}
