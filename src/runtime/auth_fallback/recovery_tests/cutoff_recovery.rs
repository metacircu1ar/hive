//! Cutoff recovery regressions.
use super::*;

#[tokio::test]
async fn repeated_cutoffs_have_bounded_identity_and_keep_original_handoff_history() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut a = core.store().agent("agent")?;
            a.auth_group = Some("plans".into());
            let mut previous = "message".to_string();
            for attempt in 1..=15 {
                let m = core.store().next_message(&a.id)?.unwrap();
                assert_eq!(m.id, previous);
                core.store().message_state(&m, MessageState::Failed)?;
                core.auth_cutoff(&mut a, &previous, "turn")?;
                core.auth_cutoff(&mut a, &previous, "turn")?;
                let next = core.store().next_message(&a.id)?.unwrap();
                assert!(next.id.len() < 100);
                assert!(next.body.contains("message message,"));
                assert!(next.body.contains(&format!("attempt {attempt}")));
                previous = next.id;
            }
            assert_eq!(
                core.store()
                    .messages(&a.id, None, 100)
                    .map(crate::api::value)?["messages"]
                    .as_array()
                    .unwrap()
                    .len(),
                16
            );
            let handoff = core.store().auth_handoff(&a.id, AuthReason::UsageLimit)?;
            assert!(handoff.contains("corpus input"));
            assert!(!handoff.contains("continuation attempt"));
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn startup_cutoff_recovery_matches_delivery_and_is_idempotent() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut a = core.store().agent("agent")?;
            a.auth_group = Some("plans".into());
            let input = core.store().next_message(&a.id)?.unwrap();
            core.store().turn_started(
                &a,
                crate::runtime::api::events::TurnStarted {
                    attachments: &[],
                    turn_id: "turn",
                    message_id: &input.id,
                    sender: "human",
                    selection: crate::runtime::api::events::TurnSelection {
                        model: &a.model,
                        effort: &a.effort,
                        options_revision: a.options_revision,
                        selection_verification: "requested_unverified",
                    },
                },
            )?;
            core.store()
                .message_state(&input, MessageState::Uncertain)?;
            a.auth_cutoff = Some(crate::runtime::domain::AuthCutoff::Bound {
                turn: "turn".into(),
                message: input.id.clone(),
            });
            a.state = AgentState::Stopped;
            a.recovery_fence = Some(crate::runtime::domain::RecoveryFence::Failure(
                "keep fence".into(),
            ));
            core.store().save(&a)?;
            core.recover_auth_cutoffs()?;
            core.recover_auth_cutoffs()?;
            let recovered = core.store().agent(&a.id)?;
            assert!(recovered.auth_cutoff.is_none());
            assert!(
                recovered
                    .auth_cutoff
                    .as_ref()
                    .and_then(crate::runtime::domain::AuthCutoff::message)
                    .is_none()
            );
            assert_eq!(recovered.recovery_fence, a.recovery_fence);
            assert_eq!(
                core.store().probe_input(&a.id, &input.id)?,
                Some(MessageState::Uncertain)
            );
            assert_eq!(
                core.store()
                    .messages(&a.id, None, 100)
                    .map(crate::api::value)?["messages"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}
