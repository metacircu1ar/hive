//! Submission, timer proof and explicit completion boundaries found in OS2 review.
use super::*;

pub(super) fn request(
    c: &mut Core,
    sender: &str,
    id: &str,
    value: serde_json::Value,
) -> serde_json::Value {
    let caller = if sender == "human" {
        crate::runtime::caller::Caller::Human
    } else {
        crate::runtime::caller::Caller::test_agent(sender)
    };
    let args = api::RequestEnvelope::new(value);
    let facts = crate::runtime::core::preflight::RequestInput::capture(c, &caller, &args).resolve();
    api::value(c.dispatch(&caller, id, &args, &facts).unwrap())
}

#[test]
fn one_shot_scheduled_followup_keeps_task_open_and_passes_delivery_proof() {
    for sender in ["human", "parent"] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        let join = setup(&mut c, true).unwrap();
        let sent = request(
            &mut c,
            sender,
            "timer",
            json!({
                "op":"send", "target":"worker", "body":"later", "delay_seconds":5
            }),
        );
        let id = sent["message_id"].as_str().unwrap();
        c.atomic(|c| {
            let (mut a, m) = start(c);
            say(c, &a, &m, "first answer", "final_answer");
            finish(c, &mut a, &m, ProviderTurnStatus::Completed)
        })
        .unwrap();
        assert_eq!(outcome(&c, &join), JoinOutcome::Pending);
        assert!(c.store().scheduled_proof(id, sender).unwrap().valid);
        c.promote_scheduled(sent["not_before"].as_u64().unwrap())
            .unwrap();
        assert_eq!(c.store().next_message("worker").unwrap().unwrap().id, id);
        c.atomic(|c| {
            let (mut a, m) = start(c);
            say(c, &a, &m, "followup answer", "final_answer");
            finish(c, &mut a, &m, ProviderTurnStatus::Completed)
        })
        .unwrap();
        let result = c.store().join_snapshot(&join).unwrap();
        assert_eq!(result.members[0].outcome, JoinOutcome::Completed);
        assert_eq!(
            result.members[0].output.as_ref().unwrap().text,
            "followup answer"
        );
    }
}

#[test]
fn one_shot_scheduled_input_without_human_proof_is_not_adopted() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    setup(&mut c, false);
    // A scheduled human message is valid only after the ordinary send path
    // explicitly tracks its call. Task output tracking must not supply that proof.
    c.store()
        .enqueue_scheduled(
            crate::runtime::store::Input::new("broken", "human", "worker", "unproved"),
            100,
        )
        .unwrap();
    assert!(!c.store().scheduled_proof("broken", "human").unwrap().valid);
    c.promote_scheduled(100).unwrap();
    let proof: Option<String> = c
        .store()
        .query_row_cached(
            "SELECT schedule_integrity FROM messages WHERE id='broken'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(proof.as_deref(), Some("invalid_delivery_proof"));
}

#[test]
fn one_shot_stopping_after_reservation_retains_the_prior_interrupted_result() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let join = setup(&mut c, true).unwrap();
    c.atomic(|c| {
        let (mut a, m) = start(c);
        say(c, &a, &m, "retained partial", "commentary");
        c.store().enqueue("next", "parent", "worker", "continue")?;
        finish(c, &mut a, &m, ProviderTurnStatus::Interrupted)
    })
    .unwrap();
    c.atomic(|c| {
        let mut a = c.store().agent("worker")?;
        a.execution_id = Some("next-execution".into());
        a.state = AgentState::Starting;
        c.store().reserve_turn(&mut a)?;
        c.store().save(&a)
    })
    .unwrap();
    assert_eq!(outcome(&c, &join), JoinOutcome::Pending);
    c.atomic(|c| {
        let mut a = c.store().agent("worker")?;
        a.paused = true;
        a.control = Some(Control::Stop);
        c.store().save(&a)
    })
    .unwrap();
    let result = c.store().join_snapshot(&join).unwrap();
    assert_eq!(result.members[0].outcome, JoinOutcome::Interrupted);
    assert_eq!(
        result.members[0].output.as_ref().unwrap().text,
        "retained partial"
    );
    assert_eq!(notices(&c).len(), 1);
    assert_eq!(
        c.store().next_message("worker").unwrap().unwrap().id,
        "next"
    );
}

#[test]
fn one_shot_explicit_completion_waits_for_claimed_submission_boundary() {
    for submitting in [false, true] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        let join = setup(&mut c, true).unwrap();
        let message = c
            .atomic(|c| {
                let mut a = c.store().agent("worker")?;
                a.state = AgentState::Starting;
                let reserved = c.store().reserve_turn(&mut a)?.unwrap();
                c.store().save(&a)?;
                c.store()
                    .execute_cached("UPDATE agents SET claimed=1 WHERE id='worker'", [])?;
                if submitting {
                    c.store()
                        .message_state(&reserved.message, MessageState::Submitting)?;
                }
                Ok(reserved.message)
            })
            .unwrap();
        request(
            &mut c,
            "human",
            "complete",
            json!({
                "op":"complete", "target":"worker", "body":"explicit result"
            }),
        );
        assert_eq!(
            c.store().agent("worker").unwrap().task_outcome,
            TaskOutcome::Open
        );
        assert_eq!(outcome(&c, &join), JoinOutcome::Pending);
        assert!(c.store().controls().unwrap().is_empty());
        assert!(notices(&c).is_empty());
        c.atomic(|c| {
            let (mut a, m) = start_message(c, message);
            say(c, &a, &m, "turn text", "final_answer");
            finish(c, &mut a, &m, ProviderTurnStatus::Completed)
        })
        .unwrap();
        let a = c.store().agent("worker").unwrap();
        assert_eq!(a.task_outcome, TaskOutcome::Completed);
        assert_eq!(a.result.as_deref(), Some("explicit result"));
        assert_eq!(a.result_selection, None);
        assert_eq!(notices(&c).len(), 1);
    }
}

#[test]
fn one_shot_idle_explicit_completion_wins_over_deferred_failure() {
    for auth_wait in [false, true] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        let join = setup(&mut c, true).unwrap();
        c.atomic(|c| {
            let (mut a, m) = start(c);
            say(c, &a, &m, "partial result", "commentary");
            if auth_wait {
                a.auth_group = Some("plans".into());
                finish(c, &mut a, &m, ProviderTurnStatus::Failed)?;
                c.auth_cutoff(&mut a, &m.id, "turn-startup")?;
                c.store().save(&a)
            } else {
                c.store().enqueue("next", "parent", "worker", "continue")?;
                finish(c, &mut a, &m, ProviderTurnStatus::Interrupted)
            }
        })
        .unwrap();
        assert_eq!(outcome(&c, &join), JoinOutcome::Pending);
        request(
            &mut c,
            "human",
            "complete",
            json!({
                "op":"complete", "target":"worker", "body":"explicit result"
            }),
        );
        assert_eq!(outcome(&c, &join), JoinOutcome::Pending);
        assert!(notices(&c).is_empty());
        // The ordinary scheduler owns idle completion, after the request commit.
        c.ready_agents(0).unwrap();
        let result = c.store().join_snapshot(&join).unwrap();
        assert_eq!(result.members[0].outcome, JoinOutcome::Completed);
        assert_eq!(
            result.members[0].output.as_ref().unwrap().text,
            "explicit result"
        );
        assert_eq!(result.members[0].output_selection, None);
        assert_eq!(notices(&c).len(), 1);
    }
}
