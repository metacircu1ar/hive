//! Quota admission regressions.
use super::*;

#[tokio::test]
async fn diagnostic_hold_wakes_fallback_but_identity_verification_does_not_clear_evidence() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut owner = core.store().agent("agent")?;
            owner.auth_group = Some("plans".into());
            owner.auth_probe_id = Some("broken".into());
            core.store().save(&owner)?;
            core.store()
                .ensure_availability("main", owner.auth_generation)?;
            let mut value = core.store().availability("main", owner.auth_generation)?;
            value.retry_at = Some(1);
            value.probe = Some(crate::runtime::auth_fallback::Probe {
                id: owner.auth_probe_id.clone().expect("probe identity"),
                agent: owner.id.clone(),
                message: "missing".into(),
                deadline: crate::state::now() + 3600,
                error: None,
            });
            core.store().save_availability(&value)?;
            let mut peer = owner.clone();
            peer.id = "peer".into();
            peer.auth_probe_id = None;
            core.store().save(&peer)?;
            core.store()
                .enqueue("peer-input", "human", &peer.id, "work")?;
            let input = core.store().next_message(&peer.id)?.unwrap();
            assert!(!core.auth_admit(&mut peer, &input)?);
            assert_eq!(peer.waiting_reason, Some(WaitingReason::Quota));
            core.reconcile_auth_probes(crate::state::now())?;
            let held = core.store().availability("main", owner.auth_generation)?;
            assert!(held.revision > value.revision);
            assert_eq!(held.probe_error(), Some(ProbeError::MissingInput));
            assert!(
                core.store()
                    .quota_waiters(r#"{"plans":["main","second"]}"#)?
                    .contains(&peer.id)
            );
            assert!(core.store().affected_auth_agents("[]")?.contains(&owner.id));
            assert!(!core.auth_admit(&mut peer, &input)?);
            assert_eq!(peer.waiting_reason, Some(WaitingReason::Switch));
            for _ in 0..2 {
                core.restore_verified_auth("main", owner.auth_generation)?;
            }
            let retained = core.store().availability("main", owner.auth_generation)?;
            assert_eq!(retained.revision, held.revision);
            assert_eq!(retained.probe_error(), held.probe_error());
            // The deliberate pinned override bypasses only admission policy.
            peer.auth_group = None;
            assert!(core.auth_admit(&mut peer, &input)?);
            assert_eq!(
                core.store()
                    .availability("main", owner.auth_generation)?
                    .probe_id(),
                held.probe_id()
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn exhausted_admission_requests_group_switch_and_irrelevant_profiles_do_not_wake_waiters() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut a = core.store().agent("agent")?;
            a.auth_group = Some("plans".into());
            let until = crate::state::now() + 86400;
            core.auth_signal(
                "main",
                a.auth_generation,
                "provider",
                signals::Signal {
                    until: Some(until),
                    ..signals::Signal::limited(AuthReason::UsageLimit)
                },
            )?;
            let input = core.store().next_message(&a.id)?.unwrap();
            assert!(!core.auth_admit(&mut a, &input)?);
            assert_eq!(a.waiting_reason, Some(WaitingReason::Switch));
            assert_eq!(a.auth_switch_pending.as_ref().unwrap().target, "plans");
            a.waiting_reason = Some(WaitingReason::Quota);
            a.quota_wait_until = Some(until);
            a.auth_checked_revision = core.store().auth_revision()?;
            core.store().save(&a)?;
            core.auth_signal(
                "unrelated",
                uuid::Uuid::new_v4(),
                "provider",
                signals::Signal::limited(AuthReason::UsageLimit),
            )?;
            assert!(
                core.store()
                    .quota_waiters(r#"{"plans":["main","second"]}"#)?
                    .is_empty()
            );
            core.auth_signal(
                "second",
                uuid::Uuid::new_v4(),
                "provider",
                signals::Signal::limited(AuthReason::UsageLimit),
            )?;
            assert_eq!(
                core.store()
                    .quota_waiters(r#"{"plans":["main","second"]}"#)?,
                vec![a.id]
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}
