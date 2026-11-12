//! Generation handoff regressions.
use super::*;

#[tokio::test]
async fn known_expiry_keeps_owner_and_clean_same_binding_unparks_without_discovery() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut a = core.store().agent("agent")?;
            a.auth_group = Some("plans".into());
            let mut row = Availability::available("main", a.auth_generation);
            row.state = AvailabilityState::Limited;
            row.until = Some(1);
            row.reason = Some(AuthReason::UsageLimit);
            row.probe = Some(crate::runtime::auth_fallback::Probe {
                id: "exact".into(),
                agent: a.id.clone(),
                message: "message".into(),
                deadline: crate::state::now() + 3600,
                error: None,
            });
            core.store().save_availability(&row)?;
            a.auth_probe_id = row.probe_id().map(str::to_owned);
            a.conversation_clean = true;
            a.state = AgentState::Stopped;
            core.request_auth_boundary(&mut a, "plans".into(), AuthReason::UsageLimit)?;
            a.waiting_reason = Some(WaitingReason::Quota);
            core.store().save(&a)?;
            core.store().expire_login_windows(2)?;
            let available = core.store().availability("main", a.auth_generation)?;
            assert_eq!(available.probe_id(), row.probe_id());
            assert_eq!(available.retry_at, None);
            assert!(core.unpark_auth_waiter(&mut a)?);
            assert!(a.waiting_reason.is_none());
            assert!(a.auth_switch_pending.is_none());
            assert_eq!(a.thread_id.as_deref(), Some("thread"));
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn verified_new_generation_is_published_once_and_wakes_old_generation_waiters() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut a = core.store().agent("agent")?;
            a.auth_group = Some("plans".into());
            core.auth_signal(
                "main",
                a.auth_generation,
                "provider",
                signals::Signal {
                    state: AvailabilityState::Unavailable,
                    ..signals::Signal::limited(AuthReason::SpendCap)
                },
            )?;
            a.waiting_reason = Some(WaitingReason::Quota);
            a.auth_checked_revision = core.store().auth_revision()?;
            core.store().save(&a)?;
            let repaired = uuid::Uuid::new_v4();
            core.restore_verified_auth("main", repaired)?;
            let published = core.store().availability("main", repaired)?;
            assert!(published.revision > a.auth_checked_revision);
            assert_eq!(published.state, AvailabilityState::Available);
            assert!(published.confirmed_at.is_none());
            assert_eq!(
                core.store().quota_waiters(r#"{"plans":["main"]}"#)?,
                vec![a.id]
            );
            core.restore_verified_auth("main", repaired)?;
            assert_eq!(
                core.store().availability("main", repaired)?.revision,
                published.revision
            );
            assert_eq!(
                core.store().availability("main", a.auth_generation)?.state,
                AvailabilityState::Unavailable
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}
