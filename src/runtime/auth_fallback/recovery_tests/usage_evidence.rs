//! Usage evidence regressions.
use super::*;

#[tokio::test]
async fn relimit_preserves_unknown_gate_and_sparse_telemetry_preserves_auth_cause() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let a = core.store().agent("agent")?;
            core.auth_signal(
                "main",
                a.auth_generation,
                "first",
                signals::Signal::limited(AuthReason::UsageLimit),
            )?;
            let old = core.store().availability("main", a.auth_generation)?;
            core.store().expire_login_windows(old.retry_at.unwrap())?;
            let known = crate::state::now() + 7200;
            core.auth_signal(
                "main",
                a.auth_generation,
                "second",
                signals::Signal {
                    until: Some(known),
                    window: Some("secondary".into()),
                    ..signals::Signal::limited(AuthReason::UsageLimit)
                },
            )?;
            let held = core.store().availability("main", a.auth_generation)?;
            assert_eq!(held.until, None);
            assert_eq!(held.retry_at, Some(known));
            core.store().expire_login_windows(known)?;
            assert!(
                core.store()
                    .availability("main", a.auth_generation)?
                    .retry_at
                    .is_some()
            );
            let auth = uuid::Uuid::new_v4();
            core.auth_signal(
                "main",
                auth,
                "provider",
                signals::Signal::limited(AuthReason::Authentication),
            )?;
            let old = core.store().availability("main", auth)?;
            core.store().expire_login_windows(old.retry_at.unwrap())?;
            let permitted = core.store().availability("main", auth)?;
            core.auth_live_read(
                crate::runtime::auth_fallback::LiveObservation {
                    profile: "main",
                    generation: auth,
                    provider: "provider",
                    revision: permitted.revision,
                    key: None,
                },
                read(json!(false)),
            )?;
            let after = core.store().availability("main", auth)?;
            assert_eq!(after.revision, permitted.revision);
            assert_eq!(after.reason, permitted.reason);
            core.restore_verified_auth("main", auth)?;
            assert!(core.store().availability("main", auth)?.retry_at.is_none());
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn auth_verification_retains_usage_and_spend_and_healthy_verification_is_idempotent() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let a = core.store().agent("agent")?;
            for reversed in [false, true] {
                let generation = uuid::Uuid::new_v4();
                let auth = signals::Signal {
                    state: AvailabilityState::Unavailable,
                    ..signals::Signal::limited(AuthReason::Authentication)
                };
                let usage = signals::Signal {
                    until: Some(crate::state::now() + 7200),
                    ..signals::Signal::limited(AuthReason::UsageLimit)
                };
                let pair = if reversed {
                    [auth, usage]
                } else {
                    [usage, auth]
                };
                for signal in pair {
                    core.auth_signal("main", generation, "provider", signal)?;
                }
                let before = core.store().availability("main", generation)?;
                assert_eq!(before.state, AvailabilityState::Unavailable);
                assert_eq!(before.reason, Some(AuthReason::UsageAndAuthentication));
                core.restore_verified_auth("main", generation)?;
                let after = core.store().availability("main", generation)?;
                assert_eq!(after.state, AvailabilityState::Limited);
                assert_eq!(after.retry_at, before.retry_at);
                assert_eq!(after.reason, before.reason);
                core.restore_verified_auth("main", generation)?;
                assert_eq!(
                    core.store().availability("main", generation)?.revision,
                    after.revision
                );
            }
            core.store()
                .ensure_availability("main", a.auth_generation)?;
            let before = core.store().availability("main", a.auth_generation)?;
            core.restore_verified_auth("main", a.auth_generation)?;
            assert_eq!(
                core.store()
                    .availability("main", a.auth_generation)?
                    .revision,
                before.revision
            );
            core.auth_signal(
                "main",
                a.auth_generation,
                "provider",
                signals::Signal {
                    state: AvailabilityState::Unavailable,
                    ..signals::Signal::limited(AuthReason::SpendCap)
                },
            )?;
            let before = core.store().availability("main", a.auth_generation)?;
            core.restore_verified_auth("main", a.auth_generation)?;
            assert_eq!(
                core.store()
                    .availability("main", a.auth_generation)?
                    .revision,
                before.revision
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[test]
fn resets_are_normalized_once_and_sparse_markers_do_not_resurrect_limits() {
    let now = 100_000;
    for bad in [
        json!(now + 33 * 86400),
        json!("sensitive raw text"),
        json!(1.5),
    ] {
        let value = limits(
            json!({"primary":{"usedPercent":100,
                "resetsAt":bad},
                "secondary":{"usedPercent":100,
                "resetsAt":now+7200},
                "rateLimitReachedType":"rate_limit_reached"}),
            now,
        );
        let signal = value.signal_at(now).unwrap();
        assert_eq!(signal.until, None);
        assert_eq!(signal.reset_floor, Some(now + 7200));
        assert_eq!(signal.rejected_resets.len(), 1);
        assert!(!format!("{:?}", signal.rejected_resets).contains("sensitive"));
    }
    let mut value = limits(
        json!({"primary":{"usedPercent":100,
            "resetsAt":now+10},
            "rateLimitReachedType":"rate_limit_reached"}),
        now,
    );
    assert!(value.signal_at(now + 40 * 86400).is_none());
    value.merge_at(
        serde_json::from_value(json!({"secondary":{"usedPercent":5},"rateLimitReachedType":null}))
            .unwrap(),
        now + 40 * 86400,
    );
    assert!(value.signal_at(now + 40 * 86400).is_none());
    for past in [json!(0), json!(-100), json!(now - 1)] {
        assert!(
            limits(
                json!({"primary":{"usedPercent":100,
            "resetsAt":past},
            "rateLimitReachedType":"rate_limit_reached"}),
                now
            )
            .signal_at(now)
            .is_none()
        );
    }
}

#[tokio::test]
async fn unknown_deadlines_are_stable_and_mixed_causes_keep_the_longer_floor() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(assert_stable_and_mixed_deadlines)
        .await
        .unwrap();
    runtime.finish();
}
