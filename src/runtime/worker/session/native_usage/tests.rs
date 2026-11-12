use super::*;
use crate::runtime::test_support::RuntimeFixture;
use serde_json::json;

fn notice(outcome: ProviderTurnStatus, tokens: u64) -> Notice {
    Notice {
        thread_id: "session".into(),
        turn_id: "turn".into(),
        outcome,
        models: serde_json::from_value(
            json!({"sonnet[1m]":{"input_tokens":tokens,"output_tokens":1,
            "cache_read_input_tokens":0,"cache_creation_input_tokens":0}}),
        )
        .unwrap(),
    }
}

#[tokio::test]
async fn auth_rebind_preserves_success_until_the_replacement_session_reports_success() {
    let fixture = RuntimeFixture::new();
    fixture
        .hub
        .with(|core| {
            let mut agent = core.store().agent("agent")?;
            agent.profile.provider = Provider::Claude;
            agent.thread_id = Some("session".into());
            agent.turn_id = Some("turn".into());
            record_snapshot(core, &mut agent, notice(ProviderTurnStatus::Completed, 10))?;
            let old = agent.native_usage.clone();
            let binding = crate::state::Binding {
                id: uuid::Uuid::new_v4(),
                profile: agent.profile.clone(),
                ready: true,
                directory: std::path::PathBuf::from("/unused"),
            };
            agent.reset_conversation(&binding);
            assert_eq!(agent.native_usage, old);
            core.store().save(&agent)?;
            agent.turn_id = Some("replacement-turn".into());
            let mut failed = notice(ProviderTurnStatus::Failed, 0);
            failed.thread_id = agent.thread_id.clone().unwrap();
            failed.turn_id = "replacement-turn".into();
            record_snapshot(core, &mut agent, failed)?;
            assert_eq!(agent.native_usage, old);
            let mut success = notice(ProviderTurnStatus::Completed, 1);
            success.thread_id = agent.thread_id.clone().unwrap();
            success.turn_id = "replacement-turn".into();
            record_snapshot(core, &mut agent, success)?;
            let snapshot = agent.native_usage.as_ref().unwrap();
            assert_eq!(snapshot.session_id, agent.thread_id.as_deref().unwrap());
            assert_eq!(snapshot.reset, Some(false));
            assert_eq!(snapshot.models.0["sonnet[1m]"].input_tokens, 1);
            Ok(())
        })
        .await
        .unwrap();
    fixture.finish();
}

#[tokio::test]
async fn usage_events_keep_failures_but_only_success_replaces_the_saved_display() {
    let fixture = RuntimeFixture::new();
    fixture
        .hub
        .with(|core| {
            let mut agent = core.store().agent("agent")?;
            agent.profile.provider = Provider::Claude;
            agent.thread_id = Some("session".into());
            agent.turn_id = Some("turn".into());
            core.store().save(&agent)
        })
        .await
        .unwrap();
    for (outcome, tokens, expected, reset) in [
        (ProviderTurnStatus::Completed, 10, 10, Some(false)),
        (ProviderTurnStatus::Failed, 0, 10, Some(false)),
        (ProviderTurnStatus::Completed, 11, 11, Some(false)),
        (ProviderTurnStatus::Completed, 1, 1, Some(true)),
    ] {
        let report = ExecutionHandle::new(&fixture.hub, "agent", "execution")
            .agent(move |core, agent| record_snapshot(core, agent, notice(outcome, tokens)))
            .await
            .unwrap();
        assert_eq!(report.is_some(), outcome == ProviderTurnStatus::Completed);
        let saved = fixture
            .hub
            .with(|core| core.store().agent("agent"))
            .await
            .unwrap();
        let usage = saved.native_usage.unwrap();
        assert_eq!(usage.models.0["sonnet[1m]"].input_tokens, expected);
        assert_eq!(usage.reset, reset);
    }
    let stale = fixture
        .hub
        .with(|core| {
            let mut agent = core.store().agent("agent")?;
            agent.turn_id = Some("new-turn".into());
            record_snapshot(core, &mut agent, notice(ProviderTurnStatus::Completed, 0))
        })
        .await
        .unwrap();
    assert!(stale.is_none());
    let events = fixture
        .hub
        .with(|core| core.store().events("agent", 0, 200))
        .await
        .unwrap();
    let events = crate::api::value(events);
    let rows = events["events"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[1]["type"], "native_usage");
    assert_eq!(rows[1]["data"]["outcome"], "failed");
    assert!(rows[1]["data"]["reset"].is_null());
    fixture.finish();
}
