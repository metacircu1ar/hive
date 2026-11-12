use super::*;

fn unfinished(store: &Store, id: &str) -> Agent {
    let mut a = agent(id, None);
    a.state = AgentState::Running;
    a.execution_id = Some(format!("execution-{id}"));
    a.turn_id = Some(format!("turn-{id}"));
    a.conversation_clean = false;
    store.save(&a).unwrap();
    store
        .event_typed(
            &a,
            EventKind::ExecutionStarting,
            api::events::ExecutionStarting {
                model: &a.model,
                effort: &a.effort,
                options_revision: 1,
            },
        )
        .unwrap();
    a
}

fn count(store: &Store, sql: &str) -> i64 {
    store
        .connection
        .query_row(sql, [], |row| row.get(0))
        .unwrap()
}

#[test]
fn recovery_records_only_unfinished_executions_once_with_uncertain_turn() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let mut active = unfinished(&store, "active");
    active.completion_pending = Some("pending".into());
    store.save(&active).unwrap();
    store
        .enqueue("input", "human", "active", "attempted")
        .unwrap();
    let input = store.next_message("active").unwrap().unwrap();
    store
        .message_state(&input, MessageState::Submitted)
        .unwrap();
    seed_already_ended(&store);
    let mut old = agent("old-id", None);
    old.execution_id = Some("execution-old-id".into());
    store.save(&old).unwrap();
    store.changes();
    store.recover_agents().unwrap();
    store.recover_agents().unwrap();
    assert_recovery(&store);
}

fn seed_already_ended(store: &Store) {
    let ended = unfinished(store, "ended");
    store
        .event_typed(
            &ended,
            EventKind::ExecutionStopped,
            api::events::ExecutionStopped::Outcome {
                task_outcome: TaskOutcome::Open,
            },
        )
        .unwrap();
    store
        .connection
        .execute("UPDATE agents SET claimed=1 WHERE id='ended'", [])
        .unwrap();
}

fn assert_recovery(store: &Store) {
    assert_eq!(
        count(
            store,
            "SELECT count(*) FROM events WHERE kind='execution_stopped'"
        ),
        2
    );
    let data: String = store
        .connection
        .query_row(
            "SELECT data FROM events WHERE agent='active' AND kind='execution_stopped'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&data).unwrap(),
        json!({
            "reason":"daemon_restarted", "outcome":"uncertain", "turn_id":"turn-active"
        })
    );
    let a = store.agent("active").unwrap();
    assert_eq!(a.state, AgentState::Stopped);
    assert!(a.turn_id.is_none() && a.last_finished_turn.is_none());
    assert!(a.completion_pending.is_none());
    assert_eq!(a.task_outcome, TaskOutcome::Open);
    assert!(!a.conversation_clean);
    let delivery: String = store
        .connection
        .query_row("SELECT state FROM messages WHERE id='input'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(delivery, "uncertain");
    assert!(store.next_message("active").unwrap().is_none());
    assert_eq!(
        count(
            store,
            "SELECT count(*) FROM events WHERE kind IN ('turn_finished','task_completed')"
        ),
        0
    );
    assert!(store.changes().observed.agents.contains("active"));
}

#[test]
fn recovery_preserves_fences_but_frees_terminal_capacity_and_wakes_clean_failures() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    for (id, terminal, fenced, blocked) in [
        ("terminal", true, true, false),
        ("fenced", false, true, false),
        ("clean", false, false, false),
        ("blocked", false, false, true),
    ] {
        let mut a = agent(id, None);
        a.state = AgentState::Failed;
        a.conversation_clean = !fenced;
        if terminal {
            a.task_outcome = TaskOutcome::Completed;
        }
        if fenced {
            a.recovery_fence =
                Some(crate::runtime::domain::RecoveryFence::ExecutionCleanupUncertain);
        }
        store.save(&a).unwrap();
        if blocked {
            store
                .connection
                .execute("UPDATE agents SET blocked=1 WHERE id=?1", [id])
                .unwrap();
        }
        store.enqueue(id, "human", id, "queued").unwrap();
    }
    assert_eq!(store.active_count().unwrap(), 4);
    store.recover_agents().unwrap();
    assert_eq!(store.active_count().unwrap(), 3);
    assert_eq!(store.agent("terminal").unwrap().state, AgentState::Stopped);
    assert!(store.agent("terminal").unwrap().recovery_fence.is_some());
    let blocker = store
        .purge_blocker(
            &store.agent("terminal").unwrap(),
            super::super::purge::CleanupState {
                claimed: false,
                blocked: true,
            },
        )
        .unwrap();
    assert_eq!(
        blocker,
        Some(crate::runtime::domain::PurgeBlockerReason::AgentCleanupUnconfirmed)
    );
    assert_eq!(
        count(&store, "SELECT runnable FROM agents WHERE id='clean'"),
        1
    );
    for id in ["terminal", "fenced", "blocked"] {
        let row: (bool, bool) = store
            .connection
            .query_row(
                "SELECT blocked,runnable FROM agents WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, (true, false), "{id}");
    }
}

#[test]
fn recovery_pages_and_rolls_back_endings_with_state_and_message_changes() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    store
        .atomic(|store| {
            for index in 0..(CHILD_PAGE_ROWS + 3) {
                unfinished(store, &format!("a-{index}"));
            }
            Ok(())
        })
        .unwrap();
    store
        .connection
        .execute_batch(
            "CREATE TEMP TRIGGER fail_recovery BEFORE INSERT ON events \
        WHEN NEW.kind='execution_stopped' AND NEW.agent='a-129' \
        BEGIN SELECT RAISE(ABORT,'injected recovery failure'); END;",
        )
        .unwrap();
    assert_eq!(store.recover_agents().unwrap_err().code(), "database_error");
    assert_eq!(
        count(
            &store,
            "SELECT count(*) FROM events WHERE kind='execution_stopped'"
        ),
        0
    );
    assert_eq!(
        count(&store, "SELECT count(*) FROM agents WHERE state='running'"),
        131
    );
    store
        .connection
        .execute_batch("DROP TRIGGER fail_recovery")
        .unwrap();
    store.recover_agents().unwrap();
    assert_eq!(
        count(
            &store,
            "SELECT count(*) FROM events WHERE kind='execution_stopped'"
        ),
        131
    );
    assert_eq!(
        count(&store, "SELECT count(*) FROM agents WHERE state='stopped'"),
        131
    );
    store.recover_agents().unwrap();
    assert_eq!(
        count(
            &store,
            "SELECT count(*) FROM events WHERE kind='execution_stopped'"
        ),
        131
    );
}
