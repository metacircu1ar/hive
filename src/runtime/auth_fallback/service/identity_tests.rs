use super::*;
use crate::runtime::{
    api::RequestEnvelope, caller::Caller, preflight::request::Facts, provider::protocol::Model,
    test_support::RuntimeFixture,
};
use serde_json::json;

fn manual(core: &mut Core, request_id: &str) -> Result<super::super::SwitchIntent> {
    let agent = core.store().agent("agent")?;
    core.config_mut()
        .auth_profiles
        .insert("second".into(), agent.profile.clone());
    core.state().register("second", &agent.profile)?;
    let mut prepared = prepare::Input::capture(core, "second".into()).resolve()?;
    for member in &mut prepared.members {
        member.catalog = Some(Ok(vec![Model::parse(&json!({"model":agent.model,
            "supportedReasoningEfforts":[{"reasoningEffort":agent.effort}]}))]));
    }
    let mut facts = Facts::default();
    facts.auth = Some(prepared);
    let request = RequestEnvelope::new(json!({"op":"set_auth","target":"agent","auth":"second"}));
    core.dispatch(&Caller::Human, request_id, &request, &facts)?;
    Ok(core.store().agent("agent")?.auth_switch_pending.unwrap())
}

#[tokio::test]
async fn replacement_intent_fences_old_success_failure_and_backoff_even_for_the_same_target() {
    let fixture = RuntimeFixture::new();
    let (old, current) = fixture
        .hub
        .with(|core| {
            let first = manual(core, "first")?;
            let before = core.store().agent("agent")?;
            assert_eq!(manual(core, "first")?.id, first.id);
            let newer = manual(core, "newer")?;
            assert_ne!(first.id, newer.id);
            let mut agent = core.store().agent("agent")?;
            core.request_auth_boundary(&mut agent, newer.target.clone(), newer.reason)?;
            assert_eq!(agent.auth_switch_pending.as_ref().unwrap().id, newer.id);
            let prepared = prepare::Input::capture(core, "second".into()).resolve()?;
            assert_eq!(
                apply_switch(core, &before, &prepared).unwrap_err().code(),
                "agent_busy"
            );
            Ok((
                SwitchWork {
                    agent: agent.id.clone(),
                    intent: first.id,
                    options_revision: agent.options_revision,
                },
                SwitchWork {
                    agent: agent.id,
                    intent: newer.id,
                    options_revision: agent.options_revision,
                },
            ))
        })
        .await
        .unwrap();
    finish_switch(&fixture.hub, &fixture.pool(), &old)
        .await
        .unwrap();
    let error = Error::process_variant_limit("old attempt must not pause replacement");
    assert!(!record_deferred(&fixture.hub, &old, &error).await.unwrap());
    let mut retries = std::collections::BTreeMap::new();
    defer(&mut retries, Job::Switch(old.clone()));
    defer(&mut retries, Job::Switch(current.clone()));
    prune_retries(&fixture.hub, &mut retries, &mut None)
        .await
        .unwrap();
    assert!(!retries.contains_key(&Job::Switch(old)));
    assert!(retries.contains_key(&Job::Switch(current.clone())));
    fixture
        .hub
        .with(move |core| {
            let agent = core.store().agent("agent")?;
            assert!(!agent.paused);
            assert!(agent.error.is_none());
            assert_eq!(
                agent.auth_switch_pending.as_ref().unwrap().id,
                current.intent
            );
            Ok(())
        })
        .await
        .unwrap();
    fixture.finish();
}

#[test]
fn switch_intents_require_their_attempt_id_in_the_current_draft_shape() {
    let intent = super::super::SwitchIntent {
        id: uuid::Uuid::new_v4(),
        target: "main".into(),
        reason: AuthReason::Manual,
        source_generation: uuid::Uuid::nil(),
        source_execution: None,
        continuation: None,
    };
    let mut value = serde_json::to_value(&intent).unwrap();
    assert_eq!(
        serde_json::from_value::<super::super::SwitchIntent>(value.clone()).unwrap(),
        intent
    );
    value.as_object_mut().unwrap().remove("id");
    assert!(serde_json::from_value::<super::super::SwitchIntent>(value).is_err());
}

#[tokio::test]
async fn an_options_correction_is_not_failed_by_an_older_switch_attempt() {
    let fixture = RuntimeFixture::new();
    let (work, current, error) = fixture
        .hub
        .with(|core| {
            let (work, error) = old_capacity_error(core)?;
            let agent = core.store().agent("agent")?;
            let mut facts = Facts::default();
            facts.model_catalog = Some(vec![Model::parse(&json!({"model":agent.model,
            "supportedReasoningEfforts":[{"reasoningEffort":agent.effort}]}))]);
            let request = RequestEnvelope::new(json!({"op":"set_options", "target":"agent",
            "options":{"codex.request_compression":true}}));
            core.dispatch(&Caller::Human, "correct-options", &request, &facts)?;
            assert!(core.store().agent("agent")?.options_revision > agent.options_revision);
            let current = switch_work(core, vec![agent.id])?.pop().unwrap();
            assert_eq!(work.intent, current.intent);
            assert_ne!(work.options_revision, current.options_revision);
            Ok((work, current, error))
        })
        .await
        .unwrap();
    finish_switch(&fixture.hub, &fixture.pool(), &work)
        .await
        .unwrap();
    assert!(!record_deferred(&fixture.hub, &work, &error).await.unwrap());
    let mut retries = std::collections::BTreeMap::new();
    defer(&mut retries, Job::Switch(work.clone()));
    defer(&mut retries, Job::Switch(current.clone()));
    prune_retries(&fixture.hub, &mut retries, &mut None)
        .await
        .unwrap();
    assert!(!retries.contains_key(&Job::Switch(work)));
    assert!(retries.contains_key(&Job::Switch(current)));
    let agent = fixture
        .hub
        .with(|core| core.store().agent("agent"))
        .await
        .unwrap();
    assert!(!agent.paused);
    assert!(agent.auth_switch_pending.is_some());
    assert!(agent.error.is_none());
    fixture.finish();
}

fn old_capacity_error(core: &mut Core) -> Result<(SwitchWork, Error)> {
    let intent = manual(core, "switch")?;
    let mut before = core.store().agent("agent")?;
    before.state = AgentState::Stopped;
    before.turn_id = None;
    before.conversation_clean = true;
    core.store().save(&before)?;
    let mut peer = before.clone();
    peer.id = "peer".into();
    peer.auth = "second".into();
    peer.auth_generation = core.state().lookup("second", &peer.profile)?.unwrap().id;
    peer.agent_options = peer.agent_options.changed(
        peer.profile.provider,
        &[("codex.request_compression".into(), json!(true))].into(),
        None,
    )?;
    core.store().save(&peer)?;
    core.config_mut().runtime.max_process_variants = 1;
    let mut prepared = prepare::Input::capture(core, "second".into())
        .after_cleanup(before.clone())
        .resolve()?;
    for member in &mut prepared.members {
        member.catalog = Some(Ok(vec![Model::parse(&json!({"model":before.model,
            "supportedReasoningEfforts":[{"reasoningEffort":before.effort}]}))]));
    }
    let error = core
        .atomic(|core| apply_switch(core, &before, &prepared))
        .unwrap_err();
    assert_eq!(error.code(), "process_variant_limit");
    Ok((
        SwitchWork {
            agent: before.id,
            intent: intent.id,
            options_revision: before.options_revision,
        },
        error,
    ))
}
