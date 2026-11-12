use super::*;
use crate::runtime::test_support::{AgentBuilder, CoreBuilder};

#[test]
fn idle_endings_stop_the_agent_without_erasing_cleanup_fences() {
    let directory = crate::runtime::test_directory();
    let mut core = CoreBuilder::new(directory.path()).build();
    for state in [AgentState::Starting, AgentState::Failed, AgentState::Idle] {
        for ending in [
            TaskEnd::Completed("done", None),
            TaskEnd::Terminated(&Caller::Human),
        ] {
            let mut agent = AgentBuilder::new("agent")
                .configure(|agent| {
                    agent.state = state;
                    agent.recovery_fence =
                        Some(crate::runtime::domain::RecoveryFence::ExecutionCleanupUncertain);
                    agent.branch_hold = 1;
                })
                .build();
            core.atomic(|core| core.end_task(&mut agent, ending))
                .unwrap();
            let saved = core.store().agent("agent").unwrap();
            assert_eq!(saved.state, AgentState::Stopped);
            assert!(saved.control.is_none());
            assert_eq!(saved.recovery_fence, agent.recovery_fence);
            assert!(!saved.conversation_clean);
            // SQL derives branch_hold from real branch intents, not this injected field.
            assert_eq!(agent.branch_hold, 1);
        }
    }
}

#[test]
fn an_owned_ending_retains_stop_until_the_worker_releases_it() {
    let directory = crate::runtime::test_directory();
    let mut core = CoreBuilder::new(directory.path()).build();
    let mut agent = AgentBuilder::new("agent")
        .configure(|a| a.state = AgentState::Running)
        .build();
    core.started(agent.id.clone());
    core.atomic(|core| core.end_task(&mut agent, TaskEnd::Terminated(&Caller::Human)))
        .unwrap();
    assert_eq!(agent.state, AgentState::Running);
    assert_eq!(agent.control, Some(Control::Stop));
}

#[test]
fn child_completion_notice_keeps_exact_full_and_truncated_text() {
    assert_eq!(
        completion_notice("日本語\nresult", None),
        "Child task completed: 日本語\nresult"
    );
    assert_eq!(
        completion_notice("日本語\nresult", Some("artifact:abc")),
        "Child task completed: 日本語\nresult\n\
        [Truncated; read full output with hive_read target=artifact:abc]"
    );
}
