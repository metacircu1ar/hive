//! Requester cleanup cannot discard a surviving task's retained or queued output.
use super::{boundaries::request, *};

fn remove_parent(c: &mut Core, gc: bool) {
    request(
        c,
        "human",
        "end-parent",
        json!({"op":"terminate", "target":"parent"}),
    );
    c.ready_agents(0).unwrap();
    let operation = if gc {
        json!({"op":"gc", "before":state::now()+1})
    } else {
        json!({"op":"purge", "target":"parent"})
    };
    request(c, "human", "remove-parent", operation);
    assert_eq!(
        c.store().agent("parent").unwrap_err().code(),
        "agent_purged"
    );
    assert_eq!(
        c.store().agent("worker").unwrap().task_outcome,
        TaskOutcome::Open
    );
}

#[test]
fn one_shot_requester_purge_and_gc_preserve_active_output_and_recovery() {
    for gc in [false, true] {
        for status in [
            ProviderTurnStatus::Completed,
            ProviderTurnStatus::Failed,
            ProviderTurnStatus::Interrupted,
        ] {
            let temp = crate::runtime::test_directory();
            let mut c = core(temp.path());
            setup(&mut c, true);
            let text = "é".repeat(40_000);
            let message = c
                .atomic(|c| {
                    let (a, m) = start(c);
                    say(c, &a, &m, &text, "final_answer");
                    Ok(m)
                })
                .unwrap();
            remove_parent(&mut c, gc);
            assert_eq!(c.store().selected_text(&message.id).unwrap().0, text);
            c.atomic(|c| {
                let mut a = c.store().agent("worker")?;
                finish(c, &mut a, &message, status)
            })
            .unwrap();
            let a = c.store().agent("worker").unwrap();
            if status == ProviderTurnStatus::Completed {
                assert_eq!(c.store().completion_text(&a).unwrap(), text);
                assert_eq!(a.task_outcome, TaskOutcome::Completed);
            } else {
                assert_eq!(a.task_outcome, TaskOutcome::Open);
            }
            drop(c);
            // These are the exact startup store transaction operations; settlement
            // must not require the deleted requester's join or call ownership.
            let s = Store::open(&temp.path().join("runtime.sqlite")).unwrap();
            s.begin().unwrap();
            s.recover_agents().unwrap();
            s.commit().unwrap();
            assert_eq!(s.agent("worker").unwrap().task_outcome, a.task_outcome);
        }
    }
}

#[test]
fn one_shot_requester_cleanup_preserves_unsubmitted_work_and_crash_output() {
    for gc in [false, true] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        setup(&mut c, true);
        remove_parent(&mut c, gc);
        let message = c
            .atomic(|c| {
                let (a, m) = start(c);
                say(c, &a, &m, "crash partial", "commentary");
                Ok(m)
            })
            .unwrap();
        assert_eq!(
            c.store().selected_text(&message.id).unwrap().0,
            "crash partial"
        );
        drop(c);
        let s = Store::open(&temp.path().join("runtime.sqlite")).unwrap();
        s.begin().unwrap();
        s.recover_agents().unwrap();
        s.commit().unwrap();
        assert_eq!(
            s.task_turn("worker").unwrap().unwrap().outcome.as_deref(),
            Some("uncertain")
        );
        assert_eq!(s.selected_text(&message.id).unwrap().0, "crash partial");
    }
}

#[test]
fn one_shot_requester_cleanup_keeps_queued_join_input_but_not_scheduled_join_proof() {
    for scheduled in [false, true] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        setup(&mut c, true);
        c.atomic(|c| {
            if scheduled {
                c.store().enqueue_scheduled(
                    crate::runtime::store::Input::new("followup", "parent", "worker", "next"),
                    100,
                )?;
            } else {
                c.store().enqueue("followup", "parent", "worker", "next")?;
            }
            let join = c.store().create_join("parent", "followup-join", None)?;
            c.store().join_member(
                &join,
                "parent",
                crate::runtime::store::MemberInput {
                    message: "followup",
                    position: 0,
                    task_scoped: false,
                },
            )?;
            let (mut a, m) = start(c);
            say(c, &a, &m, "first", "final_answer");
            finish(c, &mut a, &m, ProviderTurnStatus::Completed)
        })
        .unwrap();
        remove_parent(&mut c, false);
        if scheduled {
            assert!(
                c.store()
                    .scheduled_proof("followup", "parent")
                    .unwrap()
                    .valid
            );
            c.promote_scheduled(100).unwrap();
        }
        c.atomic(|c| {
            let (mut a, m) = start(c);
            assert_eq!(m.id, "followup");
            say(c, &a, &m, "surviving result", "final_answer");
            finish(c, &mut a, &m, ProviderTurnStatus::Completed)
        })
        .unwrap();
        assert_eq!(
            c.store().agent("worker").unwrap().result.as_deref(),
            Some("surviving result")
        );
    }
}

#[test]
fn one_shot_settled_join_does_not_reread_unneeded_missing_partial_output() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let join = setup(&mut c, true).unwrap();
    c.atomic(|c| {
        let (mut a, m) = start(c);
        say(c, &a, &m, "retained in join", "commentary");
        finish(c, &mut a, &m, ProviderTurnStatus::Failed)
    })
    .unwrap();
    assert_eq!(outcome(&c, &join), JoinOutcome::Failed);
    c.atomic(|c| {
        // Inject missing call metadata after independent join output is durable.
        c.store()
            .execute_cached("DELETE FROM call_results WHERE message_id='startup'", [])?;
        let mut a = c.store().agent("worker")?;
        a.paused = true;
        c.store().save(&a)
    })
    .unwrap();
    assert_eq!(outcome(&c, &join), JoinOutcome::Failed);
    assert_eq!(
        c.store().join_snapshot(&join).unwrap().members[0]
            .output
            .as_ref()
            .unwrap()
            .text,
        "retained in join"
    );
}
