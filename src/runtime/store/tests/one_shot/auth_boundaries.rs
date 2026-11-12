//! Auth for a future turn cannot suppress completion or exhaust a recovering task.
use super::*;
use crate::runtime::auth_fallback::{AuthReason, Probe, WaitingReason};

#[test]
fn one_shot_success_completes_before_a_future_login_boundary() {
    for reason in [AuthReason::UsageLimit, AuthReason::Manual] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        let join = setup(&mut c, true).unwrap();
        c.atomic(|c| {
            let (mut a, m) = start(c);
            a.auth_group = Some("plans".into());
            c.request_auth_boundary(&mut a, "plans".into(), reason)?;
            assert_eq!(a.waiting_reason, Some(WaitingReason::Switch));
            say(c, &a, &m, "finished review", "final_answer");
            finish(c, &mut a, &m, ProviderTurnStatus::Completed)
        })
        .unwrap();
        let a = c.store().agent("worker").unwrap();
        assert_eq!(a.task_outcome, TaskOutcome::Completed);
        assert_eq!(a.result.as_deref(), Some("finished review"));
        assert_eq!(outcome(&c, &join), JoinOutcome::Completed);
        assert_eq!(notices(&c).len(), 1);
        assert!(c.store().auth_switches_after("").unwrap().is_empty());
    }
}

#[test]
fn one_shot_quota_parked_live_session_keeps_its_automatic_continuation() {
    for fence in [false, true] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        let join = setup(&mut c, true).unwrap();
        interrupt_with_followup(&mut c);
        c.atomic(|c| {
            let mut a = c.store().agent("worker")?;
            c.store().ensure_availability(&a.auth, a.auth_generation)?;
            let mut value = c.store().availability(&a.auth, a.auth_generation)?;
            c.store()
                .enqueue("probe-input", "human", "parent", "probe")?;
            let mut parent = c.store().agent("parent")?;
            parent.auth_probe_id = Some("foreign-probe".into());
            c.store().save(&parent)?;
            value.probe = Some(Probe {
                id: "foreign-probe".into(),
                agent: "parent".into(),
                message: "probe-input".into(),
                deadline: state::now() + 60,
                error: None,
            });
            c.store().save_availability(&value)?;
            let next = c.store().next_message("worker")?.unwrap();
            assert!(!c.auth_admit(&mut a, &next)?);
            assert_eq!(a.waiting_reason, Some(WaitingReason::Quota));
            assert!(a.auth_switch_pending.is_none());
            assert!(!c.unpark_auth_waiter(&mut a)?);
            Ok(())
        })
        .unwrap();
        let a = c.store().agent("worker").unwrap();
        assert_eq!(
            c.store().automatic_continuation(&a).unwrap(),
            AutomaticContinuation::Waiting
        );
        assert_eq!(outcome(&c, &join), JoinOutcome::Pending);
        if fence {
            c.atomic(|c| {
                let mut a = c.store().agent("worker")?;
                a.recovery_fence = Some(crate::runtime::domain::RecoveryFence::Failure(
                    "held".into(),
                ));
                c.store().save(&a)
            })
            .unwrap();
            assert_eq!(outcome(&c, &join), JoinOutcome::Interrupted);
            assert_eq!(
                c.store()
                    .automatic_continuation(&c.store().agent("worker").unwrap())
                    .unwrap(),
                AutomaticContinuation::None
            );
        } else {
            c.atomic(|c| {
                let mut a = c.store().agent("worker")?;
                let mut value = c.store().availability(&a.auth, a.auth_generation)?;
                value.probe = None;
                c.store().save_availability(&value)?;
                c.store().record_conversation_release(&mut a, true)?;
                assert!(c.unpark_auth_waiter(&mut a)?);
                let (mut a, m) = start(c);
                say(c, &a, &m, "recovered answer", "final_answer");
                finish(c, &mut a, &m, ProviderTurnStatus::Completed)
            })
            .unwrap();
            let result = c.store().join_snapshot(&join).unwrap();
            assert_eq!(result.members[0].outcome, JoinOutcome::Completed);
            assert_eq!(
                result.members[0].output.as_ref().unwrap().text,
                "recovered answer"
            );
        }
        // The separate probe input plus exactly one joined delivery, never an
        // interrupted result followed by a second ordinary child-completion notice.
        assert_eq!(notices(&c).len(), 2);
    }
}

fn interrupt_with_followup(c: &mut Core) {
    c.atomic(|c| {
        let (mut a, m) = start(c);
        a.auth_group = Some("plans".into());
        a.native_input_attempted = true;
        a.conversation_clean = false;
        say(c, &a, &m, "interrupted partial", "commentary");
        c.store()
            .enqueue("redirect", "parent", "worker", "redirect")?;
        finish(c, &mut a, &m, ProviderTurnStatus::Interrupted)
    })
    .unwrap();
}
