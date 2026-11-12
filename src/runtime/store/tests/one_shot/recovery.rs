//! Deferred outcomes survive store reopen, branch holds and exhausted auth recovery.
use super::*;

#[test]
fn one_shot_deadline_partial_does_not_end_a_requester_with_pending_members() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    setup(&mut c, false);
    let join = c
        .atomic(|c| {
            c.store().save(&agent("other", None))?;
            c.store().enqueue("review", "worker", "other", "review")?;
            let join = c
                .store()
                .create_join("worker", "nested", Some(state::now()))?;
            c.store().join_member(
                &join,
                "worker",
                crate::runtime::store::MemberInput {
                    message: "review",
                    position: 0,
                    task_scoped: false,
                },
            )?;
            c.store().expire_join_deadlines(state::now())?;
            let (mut a, m) = start(c);
            say(c, &a, &m, "startup", "final_answer");
            finish(c, &mut a, &m, ProviderTurnStatus::Completed)?;
            Ok(join)
        })
        .unwrap();
    c.atomic(|c| {
        let (mut a, m) = start(c);
        say(c, &a, &m, "deadline read", "final_answer");
        finish(c, &mut a, &m, ProviderTurnStatus::Completed)
    })
    .unwrap();
    let a = c.store().agent("worker").unwrap();
    assert_eq!(a.task_outcome, TaskOutcome::Open);
    assert_eq!(
        c.store().one_shot_waiting(&a).unwrap(),
        OneShotWaiting::Join
    );
    assert!(!c.store().join_blocks_completion("worker").unwrap());
    c.atomic(|c| {
        let m = c.store().next_message("other")?.unwrap();
        c.store().message_state(&m, MessageState::Completed)
    })
    .unwrap();
    assert_eq!(
        c.store().join_snapshot(&join).unwrap().state,
        JoinState::Completed
    );
    c.atomic(|c| {
        let (mut a, m) = start(c);
        say(c, &a, &m, "final joined answer", "final_answer");
        finish(c, &mut a, &m, ProviderTurnStatus::Completed)
    })
    .unwrap();
    assert_eq!(
        c.store().agent("worker").unwrap().result.as_deref(),
        Some("final joined answer")
    );
}

#[test]
fn one_shot_bound_cutoff_survives_first_recovery_commit_and_branch_hold() {
    for grouped in [false, true] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        let join = setup(&mut c, true).unwrap();
        c.atomic(|c| {
            let (mut a, m) = start(c);
            say(c, &a, &m, "cutoff text", "commentary");
            if grouped {
                a.auth_group = Some("plans".into());
            } else {
                a.profile.method = crate::config::Method::ApiKey;
                a.profile.api_key_env = Some("HIVE_TEST_KEY".into());
            }
            a.auth_cutoff = Some(crate::runtime::domain::AuthCutoff::Bound {
                turn: a.turn_id.clone().unwrap(),
                message: m.id.clone(),
            });
            // This is startup's uncertainty transaction before auth reconciliation.
            c.store().message_state(&m, MessageState::Uncertain)?;
            a.state = AgentState::Stopped;
            a.turn_id = None;
            c.store().save(&a)?;
            c.store()
                .execute_cached("UPDATE agents SET branch_hold=1 WHERE id='worker'", [])?;
            Ok(())
        })
        .unwrap();
        assert_eq!(outcome(&c, &join), JoinOutcome::Pending);
        // Reopen a read connection: the pending result and exact cutoff are durable.
        let reopened = Store::open(&temp.path().join("runtime.sqlite")).unwrap();
        assert_eq!(
            reopened.join_snapshot(&join).unwrap().members[0].outcome,
            JoinOutcome::Pending
        );
        drop(reopened);
        c.atomic(|c| {
            let mut a = c.store().agent("worker")?;
            c.auth_uncertain_cutoff(&mut a)?;
            c.store().save(&a)
        })
        .unwrap();
        let a = c.store().agent("worker").unwrap();
        let intent = a.auth_switch_pending.clone().unwrap();
        assert_eq!(intent.target, if grouped { "plans" } else { "main" });
        assert_eq!(intent.continuation.as_ref().unwrap().input, "startup");
        assert_eq!(intent.continuation.as_ref().unwrap().turn, "turn-startup");
        assert_eq!(
            c.store().automatic_continuation(&a).unwrap(),
            AutomaticContinuation::Waiting
        );
        assert!(c.store().auth_switches_after("").unwrap().is_empty());
        c.atomic(|c| {
            c.store()
                .execute_cached("UPDATE agents SET branch_hold=0 WHERE id='worker'", [])?;
            Ok(())
        })
        .unwrap();
        assert_eq!(c.store().auth_switches_after("").unwrap(), ["worker"]);
        assert_eq!(
            c.store().agent("worker").unwrap().auth_switch_pending,
            Some(intent)
        );
        assert_eq!(outcome(&c, &join), JoinOutcome::Pending);
        c.atomic(|c| {
            let mut a = c.store().agent("worker")?;
            a.paused = true;
            c.store().save(&a)
        })
        .unwrap();
        let a = c.store().agent("worker").unwrap();
        assert!(a.auth_switch_pending.is_none());
        assert_eq!(outcome(&c, &join), JoinOutcome::Uncertain);
        assert_eq!(
            c.store().join_snapshot(&join).unwrap().members[0]
                .output
                .as_ref()
                .unwrap()
                .text,
            "cutoff text"
        );
    }
}

#[test]
fn one_shot_failed_auth_continuation_is_tracked_and_settles_when_cancelled() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let join = setup(&mut c, true).unwrap();
    c.atomic(|c| {
        let (mut a, m) = start(c);
        say(c, &a, &m, "first partial", "commentary");
        a.auth_group = Some("plans".into());
        finish(c, &mut a, &m, ProviderTurnStatus::Failed)?;
        c.auth_cutoff(&mut a, &m.id, "turn-startup")?;
        c.store().save(&a)
    })
    .unwrap();
    assert_eq!(outcome(&c, &join), JoinOutcome::Pending);
    c.atomic(|c| {
        let mut a = c.store().agent("worker")?;
        a.clear_auth_wait(0);
        a.state = AgentState::Stopped;
        c.store().save(&a)?;
        let (mut a, m) = start(c);
        assert!(m.id.starts_with("continuation:"));
        say(c, &a, &m, "continuation answer", "final_answer");
        finish(c, &mut a, &m, ProviderTurnStatus::Completed)
    })
    .unwrap();
    assert_eq!(
        c.store().agent("worker").unwrap().result.as_deref(),
        Some("continuation answer")
    );
    assert_eq!(outcome(&c, &join), JoinOutcome::Completed);
}

#[test]
fn one_shot_later_success_and_pause_never_resurrect_old_interruption() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let join = setup(&mut c, true).unwrap();
    c.atomic(|c| {
        let (mut a, m) = start(c);
        say(c, &a, &m, "obsolete partial", "commentary");
        c.store().enqueue("next", "parent", "worker", "next")?;
        finish(c, &mut a, &m, ProviderTurnStatus::Interrupted)
    })
    .unwrap();
    c.atomic(|c| {
        c.store().save(&agent("child", Some("worker")))?;
        let (mut a, m) = start(c);
        say(c, &a, &m, "new success", "final_answer");
        finish(c, &mut a, &m, ProviderTurnStatus::Completed)
    })
    .unwrap();
    c.atomic(|c| {
        let mut a = c.store().agent("worker")?;
        a.paused = true;
        c.store().save(&a)
    })
    .unwrap();
    assert_eq!(outcome(&c, &join), JoinOutcome::Pending);
    assert!(
        c.store().join_snapshot(&join).unwrap().members[0]
            .output
            .is_none()
    );
}

#[test]
fn one_shot_retains_latest_output_through_compaction_and_owns_overflow_copies() {
    for joined in [false, true] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        let join = setup(&mut c, joined);
        let text = "é".repeat(40_000);
        c.atomic(|c| {
            let (mut a, m) = start(c);
            say(c, &a, &m, &text, "final_answer");
            a.paused = true;
            finish(c, &mut a, &m, ProviderTurnStatus::Completed)
        })
        .unwrap();
        c.store()
            .execute_cached("UPDATE call_results SET accepted_at=0", [])
            .unwrap();
        assert_eq!(c.store().prune_calls(1).unwrap(), 0);
        c.atomic(|c| {
            let mut a = c.store().agent("worker")?;
            a.paused = false;
            c.store().save(&a)
        })
        .unwrap();
        let a = c.store().agent("worker").unwrap();
        assert_eq!(a.result.as_ref().unwrap().len(), 65_536);
        assert_eq!(c.store().completion_text(&a).unwrap(), text);
        let worker = a.result_read_target.unwrap();
        let copy = if let Some(join) = join {
            c.store().join_snapshot(&join).unwrap().members[0]
                .output
                .as_ref()
                .unwrap()
                .read_target
                .clone()
                .unwrap()
        } else {
            let body = &notices(&c)[0];
            let start = body.find("artifact:").unwrap();
            body[start..].trim_end_matches(']').to_owned()
        };
        assert_ne!(worker, copy);
        let id = copy.strip_prefix("artifact:").unwrap();
        assert_eq!(
            c.store()
                .query_row_cached("SELECT agent FROM artifacts WHERE id=?1", [id], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "parent"
        );
        // Worker-owned history disappearing cannot invalidate the retained copy.
        c.store()
            .execute_cached("DELETE FROM artifacts WHERE agent='worker'", [])
            .unwrap();
        assert!(!c.store().output(id, 0, 1024).unwrap().text.is_empty());
    }
}
