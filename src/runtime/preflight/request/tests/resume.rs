use super::super::*;
use crate::runtime::{
    domain::AgentState,
    test_support::{AgentBuilder, CoreBuilder},
};

fn proof(agent: &Agent) -> Facts {
    let mut facts = Facts::default();
    facts.resumes.insert(
        agent.id.clone(),
        ResumeFacts {
            agent: Ok(Identity::from(agent)),
            checked: Ok(()),
        },
    );
    facts
}

#[test]
fn resume_refusal_precedence_preserves_ownership_and_requires_proof_before_acceptance() {
    for (state, running, expected) in [
        (AgentState::Starting, true, "agent_busy"),
        (AgentState::Running, true, "agent_busy"),
        (AgentState::Stopping, true, "agent_busy"),
        (AgentState::Failed, true, "agent_releasing"),
        (AgentState::Stopped, true, "agent_releasing"),
        (AgentState::Stopped, false, "recovery_required"),
        (AgentState::Idle, false, "recovery_required"),
    ] {
        let directory = crate::runtime::test_directory();
        let mut core = CoreBuilder::new(directory.path()).build();
        let agent = AgentBuilder::new("agent")
            .configure(|a| {
                a.state = state;
                a.paused = true;
                a.conversation_clean = false;
            })
            .build();
        core.store().save(&agent).unwrap();
        if running {
            core.started(agent.id.clone());
        }
        let mut facts = proof(&agent);
        facts.resumes.get_mut(&agent.id).unwrap().checked = Err(Error::recovery_required(
            "Provider cleanup could not be confirmed.",
        ));
        let before = serde_json::to_value(&agent).unwrap();
        let request = RequestEnvelope::new(serde_json::json!({"op":"resume","target":"agent"}));
        let Err(error) = core.dispatch(&Caller::Human, "resume-refusal", &request, &facts) else {
            panic!("resume unexpectedly accepted for {state:?}");
        };
        assert_eq!(error.code(), expected, "{state:?}, running={running}");
        assert_eq!(
            serde_json::to_value(core.store().agent("agent").unwrap()).unwrap(),
            before
        );
    }
}

#[test]
fn failed_execution_resume_waits_for_release_and_wakes_its_pending_deadline() {
    for state in [AgentState::Failed, AgentState::Stopped] {
        let directory = crate::runtime::test_directory();
        let mut core = CoreBuilder::new(directory.path()).build();
        let agent = AgentBuilder::new("agent")
            .configure(|a| {
                a.state = state;
                a.paused = true;
                a.waiting_reason = Some(crate::runtime::auth_fallback::WaitingReason::Quota);
            })
            .build();
        core.store().save(&agent).unwrap();
        core.started(agent.id.clone());
        core.store().changes();
        let request = RequestEnvelope::new(serde_json::json!({"op":"resume","target":"agent"}));
        assert_eq!(
            core.dispatch(&Caller::Human, "retry", &request, &proof(&agent))
                .err()
                .unwrap()
                .code(),
            "agent_releasing"
        );
        assert!(core.store().agent("agent").unwrap().paused);
        core.stopped("agent");
        core.dispatch(&Caller::Human, "retry", &request, &proof(&agent))
            .unwrap();
        let saved = core.store().agent("agent").unwrap();
        assert!(!saved.paused);
        assert_eq!(saved.state, AgentState::Starting);
        assert!(core.store().changes().deadlines);
    }
}

#[test]
fn healthy_idle_and_clean_stopped_conversations_keep_their_existing_resume_behavior() {
    let directory = crate::runtime::test_directory();
    let mut core = CoreBuilder::new(directory.path()).build();
    for (id, state) in [("idle", AgentState::Idle), ("stopped", AgentState::Stopped)] {
        let agent = AgentBuilder::new(id)
            .configure(|a| {
                a.state = state;
                a.conversation_clean = true;
            })
            .build();
        core.store().save(&agent).unwrap();
        core.started(id.into());
        let request = RequestEnvelope::new(serde_json::json!({"op":"resume","target":id}));
        core.dispatch(&Caller::Human, id, &request, &proof(&agent))
            .unwrap();
        assert_eq!(core.store().agent(id).unwrap().state, AgentState::Starting);
    }
}
