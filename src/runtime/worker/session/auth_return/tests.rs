use super::*;
mod retry;
use crate::runtime::{
    auth_fallback::Availability,
    domain::AgentState,
    test_support::{AgentBuilder, CoreBuilder, RuntimeFixture},
};

fn setup(core: &mut Core, mut agent: Agent) -> Agent {
    let profile = agent.profile.clone();
    core.state().register("main", &profile).unwrap();
    core.config_mut()
        .auth_profiles
        .insert("main".into(), profile);
    core.config_mut().auth_groups.insert(
        "plans".into(),
        crate::config::AuthGroup::ordered(vec!["main".into()]),
    );
    agent.profile.method = Method::ApiKey;
    agent.profile.api_key_env = Some("HIVE_TEST_UNUSED_API".into());
    agent.auth = "api".into();
    agent.auth_group = Some("plans".into());
    agent.auth_generation = uuid::Uuid::new_v4();
    agent.state = AgentState::Idle;
    agent.turn_id = None;
    core.store().save(&agent).unwrap();
    core.store()
        .ensure_availability("api", agent.auth_generation)
        .unwrap();
    agent
}

fn snapshot(core: &Core, agent: &Agent) -> (Candidate, prepare::Prepared) {
    let (candidate, input) = capture(core, agent).unwrap().unwrap();
    (candidate, input.resolve().unwrap())
}

#[test]
fn a_retired_confirmation_cannot_return_the_api_session_to_a_limited_current_generation() {
    let directory = crate::runtime::test_directory();
    let mut core = CoreBuilder::new(directory.path()).build();
    let mut agent = setup(&mut core, AgentBuilder::new("agent").build());
    let profile = core.config().profile("main").unwrap().clone();
    let old = core.state().lookup("main", &profile).unwrap().unwrap();
    let mut confirmed = Availability::available("main", old.id);
    confirmed.confirmed_at = Some(crate::state::now());
    core.store().save_revised(&mut confirmed).unwrap();
    let repair = core.state().begin_repair("main", &profile).unwrap();
    assert!(core.state().finish_repair(&repair, true, None).unwrap());
    let mut limited = Availability::available("main", repair.binding.id);
    limited.state = crate::runtime::auth_fallback::AvailabilityState::Limited;
    limited.reason = Some(AuthReason::UsageLimit);
    limited.until = Some(crate::state::now() + 3600);
    core.store().save_revised(&mut limited).unwrap();
    let proof = snapshot(&core, &agent);
    assert_eq!(
        proof.1.members[0].binding.as_ref().unwrap().id,
        repair.binding.id
    );
    let revision = core.store().auth_revision().unwrap();
    core.atomic(|core| apply(core, &mut agent, Some(&proof)))
        .unwrap();
    assert_eq!(agent.auth, "api");
    assert!(agent.auth_switch_pending.is_none());
    assert_eq!(agent.auth_checked_revision, revision);
    limited.mark_available();
    limited.confirmed_at = Some(crate::state::now());
    core.store().save_revised(&mut limited).unwrap();
    let proof = snapshot(&core, &agent);
    core.atomic(|core| apply(core, &mut agent, Some(&proof)))
        .unwrap();
    assert_eq!(agent.auth_switch_pending.as_ref().unwrap().target, "plans");
}

#[test]
fn inconclusive_bindings_keep_the_return_check_armed_and_stale_or_manual_proofs_do_not_apply() {
    let directory = crate::runtime::test_directory();
    let mut core = CoreBuilder::new(directory.path()).build();
    let mut agent = setup(&mut core, AgentBuilder::new("agent").build());
    let mut proof = snapshot(&core, &agent);
    proof.1.members[0].binding = Err(crate::error::Error::state_io("fixture read error"));
    assert!(apply(&core, &mut agent, Some(&proof)).unwrap());
    assert_eq!(agent.auth_checked_revision, 0);
    assert!(agent.auth_switch_pending.is_none());
    let proof = snapshot(&core, &agent);
    let mut unrelated = Availability::available("other", uuid::Uuid::new_v4());
    core.store().save_revised(&mut unrelated).unwrap();
    assert!(!apply(&core, &mut agent, Some(&proof)).unwrap());
    assert_eq!(agent.auth_checked_revision, 0);
    core.request_auth_boundary(&mut agent, "manual".into(), AuthReason::Manual)
        .unwrap();
    let manual = agent.auth_switch_pending.clone();
    assert!(apply(&core, &mut agent, Some(&proof)).unwrap());
    assert_eq!(agent.auth_switch_pending, manual);
}

#[test]
fn an_unbound_claude_fork_never_attempts_an_auth_return() {
    let directory = crate::runtime::test_directory();
    let mut core = CoreBuilder::new(directory.path()).build();
    let mut agent = setup(&mut core, AgentBuilder::new("agent").build());
    assert!(needs_check(&core, &agent).unwrap());
    agent.profile.provider = crate::config::Provider::Claude;
    agent.branch_phase = Some(crate::runtime::branching::BranchPhase::Forking);
    agent.branched_from_agent_id = Some("source".into());
    assert!(agent.branch_unbound());
    assert!(!models::claim_blocked(&core, &agent));
    assert!(!needs_check(&core, &agent).unwrap());
}

#[tokio::test]
async fn final_api_admission_keeps_the_session_and_marks_only_its_input_submitting() {
    let fixture = RuntimeFixture::new();
    let loaded = fixture
        .hub
        .with(|core| {
            let original = core.store().agent("agent")?;
            let agent = setup(core, original);
            Ok(agent.boundary_selection())
        })
        .await
        .unwrap();
    let admission = claim(
        ExecutionHandle::new(&fixture.hub, "agent", "execution"),
        loaded,
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(admission, TurnAdmission::Submit { .. }));
    fixture
        .hub
        .with(|core| {
            let agent = core.store().agent("agent")?;
            assert_eq!(agent.auth, "api");
            assert_eq!(agent.thread_id.as_deref(), Some("thread"));
            assert!(agent.auth_switch_pending.is_none());
            Ok(())
        })
        .await
        .unwrap();
    fixture.finish();
}

#[tokio::test]
async fn continuously_stale_auth_snapshots_return_a_bounded_retry_without_submitting_input() {
    let fixture = RuntimeFixture::new();
    let loaded = fixture
        .hub
        .with(|core| {
            let original = core.store().agent("agent")?;
            let agent = setup(core, original);
            core.store().test_hooks().at_sql("begin", |db| {
                db.execute("UPDATE login_availability SET revision=revision+1", [])?;
                Ok(())
            });
            Ok(agent.boundary_selection())
        })
        .await
        .unwrap();
    assert!(
        claim(
            ExecutionHandle::new(&fixture.hub, "agent", "execution"),
            loaded,
            None
        )
        .await
        .unwrap()
        .is_none()
    );
    fixture
        .hub
        .with(|core| {
            let agent = core.store().agent("agent")?;
            assert_eq!(agent.auth, "api");
            assert_eq!(agent.thread_id.as_deref(), Some("thread"));
            assert!(agent.reserved_turn.is_none());
            assert!(agent.auth_switch_pending.is_none());
            assert_eq!(core.store().next_message("agent")?.unwrap().id, "message");
            Ok(())
        })
        .await
        .unwrap();
    fixture.finish();
}
