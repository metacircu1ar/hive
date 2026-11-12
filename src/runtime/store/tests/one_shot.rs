//! One-shot task boundaries exercise durable joins, pending work and recovery.
mod auth_boundaries;
mod boundaries;
mod ownership;
mod recovery;
use super::*;
use crate::runtime::{
    api::joins::{JoinOutcome, JoinState},
    branching::{AttemptedTurn, FinishedTurn},
    domain::{Control, OutputSelection, ProviderTurnStatus},
    store::{AutomaticContinuation, OneShotWaiting},
};

fn setup(c: &mut Core, joined: bool) -> Option<String> {
    c.atomic(|c| {
        c.store().save(&agent("parent", None))?;
        let mut a = agent("worker", Some("parent"));
        a.one_shot = true;
        a.state = AgentState::Idle;
        a.thread_id = Some("thread".into());
        a.execution_id = Some("execution".into());
        c.store().save(&a)?;
        c.store().enqueue("startup", "parent", "worker", "review")?;
        if joined {
            let join = c.store().create_join("parent", "round", None)?;
            c.store().join_member(
                &join,
                "parent",
                crate::runtime::store::MemberInput {
                    message: "startup",
                    position: 0,
                    task_scoped: true,
                },
            )?;
            Ok(Some(join))
        } else {
            Ok(None)
        }
    })
    .unwrap()
}
fn start(c: &Core) -> (Agent, Message) {
    let m = c.store().next_message("worker").unwrap().unwrap();
    start_message(c, m)
}
fn start_message(c: &Core, m: Message) -> (Agent, Message) {
    let mut a = c.store().agent("worker").unwrap();
    a.state = AgentState::Running;
    a.turn_id = Some(format!("turn-{}", m.id));
    a.last_attempted_turn = Some(AttemptedTurn {
        message: m.id.clone(),
        thread: "thread".into(),
        execution: "execution".into(),
    });
    c.store().save(&a).unwrap();
    c.store()
        .message_state(&m, MessageState::Submitting)
        .unwrap();
    c.store()
        .turn_started(
            &a,
            api::events::TurnStarted {
                attachments: &[],
                turn_id: a.turn_id.as_deref().unwrap(),
                message_id: &m.id,
                sender: &m.sender,
                selection: api::events::TurnSelection {
                    model: &a.model,
                    effort: &a.effort,
                    options_revision: 0,
                    selection_verification: "requested_unverified",
                },
            },
        )
        .unwrap();
    c.store()
        .message_state(&m, MessageState::Submitted)
        .unwrap();
    (a, m)
}
fn say(c: &Core, a: &Agent, m: &Message, text: &str, phase: &str) {
    c.store()
        .assistant_output(
            a,
            Some(&m.id),
            json!({"turn_id":a.turn_id,"text":text,"phase":phase}),
            true,
        )
        .unwrap();
}
fn finish(
    c: &mut Core,
    a: &mut Agent,
    m: &Message,
    outcome: ProviderTurnStatus,
) -> crate::error::Result<()> {
    let turn = a.turn_id.take().unwrap();
    c.store()
        .message_state(m, MessageState::from_provider_completion(Some(outcome)))?;
    a.state = if outcome == ProviderTurnStatus::Failed {
        AgentState::Failed
    } else {
        AgentState::Idle
    };
    a.last_finished_turn = Some(FinishedTurn {
        thread: "thread".into(),
        execution: "execution".into(),
        turn,
        message: Some(m.id.clone()),
        outcome,
        at: state::now(),
    });
    c.store().save(a)?;
    if outcome == ProviderTurnStatus::Completed {
        c.prepare_one_shot_completion(a)?;
        c.finish_task_turn(a, Some(&m.id))?;
    }
    Ok(())
}
fn outcome(c: &Core, join: &str) -> JoinOutcome {
    c.store().join_snapshot(join).unwrap().members[0].outcome
}
fn notices(c: &Core) -> Vec<String> {
    c.store()
        .sql()
        .query_column(
            "SELECT body FROM messages WHERE recipient='parent' ORDER BY seq",
            [],
        )
        .unwrap()
}

#[test]
fn one_shot_final_answer_wins_and_task_join_receives_one_owned_result() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let join = setup(&mut c, true).unwrap();
    c.atomic(|c| {
        let (mut a, m) = start(c);
        say(c, &a, &m, "working", "commentary");
        say(c, &a, &m, "finished", "final_answer");
        say(c, &a, &m, "later commentary", "commentary");
        finish(c, &mut a, &m, ProviderTurnStatus::Completed)
    })
    .unwrap();
    let a = c.store().agent("worker").unwrap();
    assert_eq!(a.result.as_deref(), Some("finished"));
    assert_eq!(a.completed_by.as_deref(), Some("one_shot"));
    assert_eq!(a.result_selection, Some(OutputSelection::FinalAnswer));
    assert_eq!(a.task_outcome, TaskOutcome::Completed);
    let snapshot = c.store().join_snapshot(&join).unwrap();
    assert_eq!(snapshot.state, JoinState::Completed);
    assert_eq!(
        snapshot.members[0].output.as_ref().unwrap().text,
        "finished"
    );
    assert!(snapshot.members[0].task_completed.is_none());
    assert_eq!(notices(&c).len(), 1);
    assert!(!notices(&c)[0].contains("Child task completed"));
    c.atomic(|_| Ok(())).unwrap();
    assert_eq!(notices(&c).len(), 1);
}

#[test]
fn one_shot_followups_from_any_sender_keep_the_join_pending() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let join = setup(&mut c, true).unwrap();
    c.atomic(|c| {
        let (mut a, m) = start(c);
        say(c, &a, &m, "first", "final_answer");
        c.store()
            .enqueue("followup", "someone-else", "worker", "more")?;
        finish(c, &mut a, &m, ProviderTurnStatus::Completed)
    })
    .unwrap();
    assert_eq!(outcome(&c, &join), JoinOutcome::Pending);
    assert!(notices(&c).is_empty());
    c.atomic(|c| {
        let (mut a, m) = start(c);
        say(c, &a, &m, "last answer", "commentary");
        finish(c, &mut a, &m, ProviderTurnStatus::Completed)
    })
    .unwrap();
    assert_eq!(
        c.store().agent("worker").unwrap().result.as_deref(),
        Some("last answer")
    );
    assert_eq!(outcome(&c, &join), JoinOutcome::Completed);
}

#[test]
fn one_shot_no_text_still_completes_with_an_explicit_parent_notice() {
    for text in [None, Some("")] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        setup(&mut c, false);
        c.atomic(|c| {
            let (mut a, m) = start(c);
            if let Some(text) = text {
                say(c, &a, &m, text, "final_answer");
            }
            finish(c, &mut a, &m, ProviderTurnStatus::Completed)
        })
        .unwrap();
        let a = c.store().agent("worker").unwrap();
        assert_eq!(a.result.as_deref(), Some(""));
        assert_eq!(a.result_selection, Some(OutputSelection::NoText));
        assert_eq!(notices(&c), ["Child task completed without final text."]);
    }
}

#[test]
fn one_shot_pause_then_idle_resume_uses_success_but_a_later_reservation_invalidates_it() {
    for later_attempt in [false, true] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        setup(&mut c, false);
        c.atomic(|c| {
            let (mut a, m) = start(c);
            say(c, &a, &m, "retained", "final_answer");
            a.paused = true;
            finish(c, &mut a, &m, ProviderTurnStatus::Completed)
        })
        .unwrap();
        assert_eq!(
            c.store().agent("worker").unwrap().task_outcome,
            TaskOutcome::Open
        );
        c.atomic(|c| {
            let mut a = c.store().agent("worker")?;
            if later_attempt {
                c.store().enqueue("new", "human", "worker", "new")?;
                c.store().reserve_turn(&mut a)?;
                c.store().save(&a)?;
                c.store().undeliverable("worker")?;
                a.reserved_turn = None;
            }
            a.paused = false;
            c.store().save(&a)
        })
        .unwrap();
        assert_eq!(
            c.store().agent("worker").unwrap().task_outcome,
            if later_attempt {
                TaskOutcome::Open
            } else {
                TaskOutcome::Completed
            }
        );
    }
}

#[test]
fn one_shot_waits_for_independent_children_and_rechecks_when_they_end() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    setup(&mut c, false);
    c.atomic(|c| {
        c.store().save(&agent("child", Some("worker")))?;
        let (mut a, m) = start(c);
        say(c, &a, &m, "coordinator answer", "final_answer");
        finish(c, &mut a, &m, ProviderTurnStatus::Completed)
    })
    .unwrap();
    assert_eq!(
        c.store()
            .one_shot_waiting(&c.store().agent("worker").unwrap())
            .unwrap(),
        OneShotWaiting::Children
    );
    c.atomic(|c| {
        let mut child = c.store().agent("child")?;
        child.task_outcome = TaskOutcome::Cancelled;
        c.store().save(&child)
    })
    .unwrap();
    assert_eq!(
        c.store().agent("worker").unwrap().task_outcome,
        TaskOutcome::Completed
    );
}

#[test]
fn one_shot_interruption_defers_for_queued_work_but_failure_does_not() {
    for status in [ProviderTurnStatus::Interrupted, ProviderTurnStatus::Failed] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        let join = setup(&mut c, true).unwrap();
        c.atomic(|c| {
            let (mut a, m) = start(c);
            say(c, &a, &m, "partial", "commentary");
            c.store().enqueue("next", "parent", "worker", "continue")?;
            finish(c, &mut a, &m, status)
        })
        .unwrap();
        assert_eq!(
            outcome(&c, &join),
            if status == ProviderTurnStatus::Interrupted {
                JoinOutcome::Pending
            } else {
                JoinOutcome::Failed
            }
        );
        c.atomic(|c| {
            let (mut a, m) = start(c);
            say(c, &a, &m, "new answer", "final_answer");
            finish(c, &mut a, &m, ProviderTurnStatus::Completed)
        })
        .unwrap();
        if status == ProviderTurnStatus::Failed {
            assert_eq!(outcome(&c, &join), JoinOutcome::Failed);
            assert_eq!(notices(&c).len(), 2);
            assert_eq!(notices(&c)[1], "Child task completed: new answer");
        } else {
            assert_eq!(outcome(&c, &join), JoinOutcome::Completed);
            assert_eq!(notices(&c).len(), 1);
        }
    }
}

#[test]
fn one_shot_deferred_failure_settles_on_pause_without_replaying_input() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let join = setup(&mut c, true).unwrap();
    c.atomic(|c| {
        let (mut a, m) = start(c);
        say(c, &a, &m, "held partial", "commentary");
        c.store().enqueue("next", "parent", "worker", "continue")?;
        finish(c, &mut a, &m, ProviderTurnStatus::Interrupted)
    })
    .unwrap();
    assert_eq!(outcome(&c, &join), JoinOutcome::Pending);
    c.atomic(|c| {
        let mut a = c.store().agent("worker")?;
        a.paused = true;
        c.store().save(&a)
    })
    .unwrap();
    let snapshot = c.store().join_snapshot(&join).unwrap();
    assert_eq!(snapshot.members[0].outcome, JoinOutcome::Interrupted);
    assert_eq!(
        snapshot.members[0].output.as_ref().unwrap().text,
        "held partial"
    );
    assert_eq!(
        c.store().next_message("worker").unwrap().unwrap().id,
        "next"
    );
}

#[test]
fn one_shot_pre_submission_failure_has_no_stale_success_output() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let join = setup(&mut c, true).unwrap();
    c.atomic(|c| {
        let (mut a, m) = start(c);
        say(c, &a, &m, "old answer", "final_answer");
        c.store().enqueue("next", "parent", "worker", "continue")?;
        finish(c, &mut a, &m, ProviderTurnStatus::Completed)
    })
    .unwrap();
    c.atomic(|c| {
        let mut a = c.store().agent("worker")?;
        a.state = AgentState::Failed;
        c.store().save(&a)
    })
    .unwrap();
    let snapshot = c.store().join_snapshot(&join).unwrap();
    assert_eq!(snapshot.members[0].outcome, JoinOutcome::Failed);
    assert!(snapshot.members[0].output.is_none());
    assert_eq!(
        snapshot.members[0].output_selection,
        Some(OutputSelection::NoText)
    );
}

#[test]
fn one_shot_task_join_idle_explicit_completion_and_ended_outcomes() {
    for ended in ["completed", "cancelled", "expired"] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        let join = setup(&mut c, true).unwrap();
        c.atomic(|c| {
            let mut a = c.store().agent("worker")?;
            if ended == "completed" {
                a.completion_pending = Some("explicit answer".into());
                a.completion_pending_by = Some("human".into());
                c.finish_task(&mut a)
            } else {
                if ended == "cancelled" {
                    a.task_outcome = TaskOutcome::Cancelled;
                } else {
                    a.expired = true;
                }
                c.store().save(&a)?;
                c.store().settle_task_end(&a)?;
                Ok(())
            }
        })
        .unwrap();
        let snapshot = c.store().join_snapshot(&join).unwrap();
        if ended == "completed" {
            assert_eq!(snapshot.members[0].outcome, JoinOutcome::Completed);
            assert_eq!(snapshot.members[0].output_selection, None);
            assert_eq!(
                snapshot.members[0].output.as_ref().unwrap().text,
                "explicit answer"
            );
            assert!(snapshot.members[0].task_completed.is_none());
        } else {
            assert_eq!(snapshot.members[0].outcome, JoinOutcome::Undeliverable);
        }
        assert_eq!(notices(&c).len(), 1);
    }
}

#[test]
fn one_shot_automatic_continuation_respects_final_admission_facts() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    setup(&mut c, false);
    let s = c.store();
    let mut a = s.agent("worker").unwrap();
    assert_eq!(
        s.automatic_continuation(&a).unwrap(),
        AutomaticContinuation::Runnable
    );
    a.recovery_fence = Some(crate::runtime::domain::RecoveryFence::Failure(
        "retained".into(),
    ));
    s.save(&a).unwrap();
    assert_eq!(
        s.automatic_continuation(&a).unwrap(),
        AutomaticContinuation::Runnable
    );
    for (column, value, expected) in [
        ("branch_hold", 1, AutomaticContinuation::Waiting),
        ("blocked", 1, AutomaticContinuation::None),
        ("claimed", 1, AutomaticContinuation::Runnable),
    ] {
        s.execute_cached(
            &format!("UPDATE agents SET {column}=?1 WHERE id='worker'"),
            [value],
        )
        .unwrap();
        assert_eq!(
            s.automatic_continuation(&s.agent("worker").unwrap())
                .unwrap(),
            expected
        );
        s.execute_cached(
            &format!("UPDATE agents SET {column}=0 WHERE id='worker'"),
            [],
        )
        .unwrap();
    }
    a.control = Some(Control::Stop);
    s.save(&a).unwrap();
    assert_eq!(
        s.automatic_continuation(&a).unwrap(),
        AutomaticContinuation::None
    );
}
