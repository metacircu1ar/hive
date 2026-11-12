use super::*;
use crate::runtime::test_support::{RuntimeFixture, failure_log};

#[test]
fn failed_claude_acquisition_distinguishes_preassigned_identity_from_native_launch() {
    use crate::runtime::domain::ProviderProcess;
    use crate::runtime::test_support::{AgentBuilder, CoreBuilder};
    let directory = crate::runtime::test_directory();
    let mut core = CoreBuilder::new(directory.path()).build();
    for id in ["fresh", "reserved", "input", "binding"] {
        let agent = AgentBuilder::new(id)
            .configure(|agent| {
                agent.profile.provider = crate::config::Provider::Claude;
                agent.execution_id = Some("execution".into());
                agent.thread_id = Some("preassigned".into());
                agent.native_input_attempted = id == "input";
                agent.provider_process = (id == "reserved").then(|| ProviderProcess::Reserved {
                    id: "provider".into(),
                });
            })
            .build();
        core.store().save(&agent).unwrap();
        core.atomic(|core| {
            if id == "binding" {
                core.store().record_native_binding(&agent, "preassigned")?;
            }
            record_failure(
                core,
                id,
                Error::provider_request_failed("native rejection"),
                true,
                Some("execution"),
            )
        })
        .unwrap();
        assert_eq!(
            core.store().agent(id).unwrap().conversation_clean,
            id == "fresh"
        );
    }
}

#[test]
fn failed_fresh_acquisition_is_clean_only_without_native_ownership_evidence() {
    use crate::runtime::test_support::{AgentBuilder, CoreBuilder};
    let directory = crate::runtime::test_directory();
    let mut core = CoreBuilder::new(directory.path()).build();
    for (id, cleaned, reserved, expected_clean) in [
        ("fresh", true, Some("execution"), true),
        ("attempt", true, Some("execution"), false),
        ("binding", true, Some("execution"), false),
        ("thread", true, Some("execution"), false),
        ("input", true, Some("execution"), false),
        ("unclean", false, Some("execution"), false),
        ("stale", true, Some("other"), false),
    ] {
        let agent = AgentBuilder::new(id)
            .configure(|agent| {
                agent.execution_id = Some("execution".into());
                agent.thread_id = (id == "thread").then(|| "thread".into());
                agent.native_input_attempted = id == "input";
            })
            .build();
        core.store().save(&agent).unwrap();
        core.atomic(|core| {
            if id == "attempt" {
                core.store().begin_native_creation(&agent)?;
            } else if id == "binding" {
                core.store().record_native_binding(&agent, "thread")?;
            }
            record_failure(
                core,
                id,
                Error::provider_request_failed("native rejection"),
                cleaned,
                reserved,
            )
        })
        .unwrap();
        let saved = core.store().agent(id).unwrap();
        assert_eq!(saved.conversation_clean, expected_clean, "{id}");
        assert_eq!(saved.recovery_fence.is_some(), !cleaned, "{id}");
    }
}

#[test]
fn safely_retried_credential_changes_are_not_logged_as_execution_errors() {
    let Some(log) = failure_log(
        module_path!(),
        "safely_retried_credential_changes_are_not_logged_as_execution_errors",
        || {
            let fixture = RuntimeFixture::new();
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(finish_failure(
                    &fixture.hub,
                    "agent".into(),
                    crate::state::key_changed(),
                    true,
                    Some("execution".into()),
                ));
            fixture
                .hub
                .blocking_with(|core| {
                    let agent = core.store().agent("agent")?;
                    assert_eq!(agent.state, AgentState::Stopped);
                    assert!(agent.recovery_fence.is_none());
                    Ok(())
                })
                .unwrap();
            fixture.finish();
        },
    ) else {
        return;
    };
    assert!(!log.contains("worker.execution_failed"));
    assert!(!log.contains("worker.failure_record_failed"));
    assert!(log.contains("agent.execution_stopped"));
}

#[test]
fn credential_retry_closes_only_its_committed_execution_reservation() {
    use crate::runtime::test_support::{AgentBuilder, CoreBuilder};
    let directory = crate::runtime::test_directory();
    let mut core = CoreBuilder::new(directory.path()).build();
    for (id, reserved, endings) in [
        ("preflight", None, 0),
        ("stale", Some("previous"), 0),
        ("reserved", Some("current"), 1),
    ] {
        let agent = AgentBuilder::new(id)
            .configure(|a| {
                a.execution_id = Some("current".into());
            })
            .build();
        core.store().save(&agent).unwrap();
        core.atomic(|core| record_failure(core, id, crate::state::key_changed(), true, reserved))
            .unwrap();
        let saved = core.store().agent(id).unwrap();
        assert_eq!(saved.state, AgentState::Stopped);
        assert_eq!(saved.conversation_clean, id == "reserved");
        let crate::runtime::api::ReadOutput::Events(page) =
            core.store().events(id, 0, 200).unwrap()
        else {
            panic!("expected event page")
        };
        assert_eq!(page.events.len(), endings);
        if let Some(event) = page.events.first() {
            assert_eq!(event.kind, EventKind::ExecutionStopped);
            assert_eq!(event.execution_id.as_deref(), Some("current"));
            assert_eq!(event.data["reason"], "credential_changed_before_submission");
        }
    }
}

#[tokio::test]
async fn an_unfinished_stop_cannot_commit_a_pending_completion_or_expire_children() {
    let fixture = RuntimeFixture::new();
    fixture
        .hub
        .with(|core| {
            let mut agent = core.store().agent("agent")?;
            agent.completion_pending = Some("not confirmed".into());
            agent.completion_pending_by = Some("human".into());
            core.store().save(&agent)?;
            let child = crate::runtime::test_support::AgentBuilder::new("child")
                .configure(|a| {
                    a.parent_id = Some("agent".into());
                    a.lifetime = crate::config::Lifetime::WithParent;
                })
                .build();
            core.store().save(&child)
        })
        .await
        .unwrap();
    record_execution_stopped(
        ExecutionHandle::new(&fixture.hub, "agent", "execution"),
        super::super::provider::Shutdown {
            method: ShutdownMethod::SessionReleased,
            confirmed: true,
            quiescent: true,
            reaped: false,
            exit_code: None,
        },
        Ok(true),
        false,
    )
    .await
    .unwrap();
    fixture
        .hub
        .with(|core| {
            let agent = core.store().agent("agent")?;
            assert!(agent.completion_pending.is_none());
            assert!(agent.completion_pending_by.is_none());
            assert!(!agent.terminal());
            assert!(!core.store().agent("child")?.terminal());
            let crate::runtime::api::ReadOutput::Events(page) =
                core.store().events("agent", 0, 200)?
            else {
                panic!("expected event page")
            };
            assert!(
                !page
                    .events
                    .iter()
                    .any(|e| e.kind == EventKind::TaskCompleted)
            );
            Ok(())
        })
        .await
        .unwrap();
    fixture.finish();
}

#[tokio::test]
async fn stop_record_and_cleanup_failures_keep_the_turn_cause_and_recovery_fence() {
    let fixture = RuntimeFixture::new();
    fixture.hub.with(|core| {
        core.store().test_connection().execute_batch(
            "CREATE TRIGGER fail_stop BEFORE INSERT ON events WHEN NEW.kind = 'provider_session_released' BEGIN SELECT RAISE(ABORT, 'private stop failure'); END;"
        ).map_err(|cause| Error::database_error("fixture trigger").with_source(cause))
    }).await.unwrap();
    let result = finish_execution(
        ExecutionHandle::new(&fixture.hub, "agent", "execution"),
        Err(Error::provider_io("private turn failure")
            .with_source(std::io::Error::from_raw_os_error(libc::EPIPE))),
        super::super::provider::Shutdown {
            method: ShutdownMethod::SessionReleased,
            confirmed: false,
            quiescent: false,
            reaped: false,
            exit_code: None,
        },
        false,
    )
    .await;
    let primary = result.unwrap_err();
    assert_eq!(primary.code(), "provider_io");
    assert_eq!(primary.io_raw_os_error(), Some(libc::EPIPE));
    fixture
        .hub
        .with(move |core| {
            // The failed stop transaction rolled back; uncertain cleanup must still
            // fence the surviving execution even though its original error wins.
            assert_eq!(core.store().agent("agent")?.state, AgentState::Running);
            assert!(!record_failure(core, "agent", primary, false, None)?);
            let agent = core.store().agent("agent")?;
            assert_eq!(agent.state, AgentState::Failed);
            assert!(agent.recovery_fence.is_some());
            assert_eq!(agent.error.as_deref(), Some("provider_io"));
            Ok(())
        })
        .await
        .unwrap();
    fixture.finish();
}

#[test]
fn failed_lifecycle_write_retains_both_causes_and_fences_without_writer_failure() {
    let Some(log) = failure_log(
        module_path!(),
        "failed_lifecycle_write_retains_both_causes_and_fences_without_writer_failure",
        || {
            let fixture = RuntimeFixture::new();
            fixture.hub.blocking_with(|core| {
            core.store().test_connection().execute_batch(
                "CREATE TRIGGER fail_lifecycle BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'secret lifecycle SQL'); END;"
            ).map_err(|cause| Error::database_error("fixture trigger").with_source(cause))
        }).unwrap();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(finish_failure(
                &fixture.hub,
                "agent".into(),
                Error::provider_request_failed("secret provider message"),
                true,
                None,
            ));
            assert_eq!(fixture.hub.wake.failure().unwrap().code(), "database_error");
            fixture
                .hub
                .blocking_with(|core| {
                    assert!(core.shutdown().load(Ordering::Acquire));
                    // The individual job rolled back cleanly; the writer itself still works.
                    assert_eq!(core.store().agent("agent")?.state, AgentState::Running);
                    Ok(())
                })
                .unwrap();
            fixture.finish();
        },
    ) else {
        return;
    };
    assert!(log.contains("worker.execution_failed"));
    assert!(log.contains("worker.failure_record_failed"));
    assert!(log.contains("provider_request_failed"));
    assert!(log.contains("database_error"));
    assert!(!log.contains("secret lifecycle SQL"));
    assert!(!log.contains("secret provider message"));
}
