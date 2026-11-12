//! Published readiness regressions.
use super::*;

#[tokio::test]
async fn join_settlement_is_included_in_published_readiness() {
    let runtime = RuntimeFixture::new();
    let started = start(&runtime).await;
    runtime
        .hub
        .with(|core| {
            let s = core.store();
            let mut requester = s.agent("agent")?;
            requester.state = crate::runtime::domain::AgentState::Stopped;
            requester.turn_id = None;
            requester.control = None;
            requester.paused = false;
            s.save(&requester)?;
            let mut worker = requester.clone();
            worker.id = "worker".into();
            s.save(&worker)?;
            let id = s.create_join("agent", "published", None)?;
            s.enqueue("member", "agent", "worker", "work")?;
            s.join_member(
                &id,
                "agent",
                crate::runtime::store::MemberInput {
                    message: "member",
                    position: 0,
                    task_scoped: false,
                },
            )?;
            s.undeliverable("worker")?;
            Ok(())
        })
        .await
        .unwrap();
    let ping = read(&started.reader, json!({"op":"ping"})).await.unwrap();
    assert_eq!(ping["storage"]["ready"], true);
    let messages = read(&started.reader, json!({"op":"messages","target":"agent"}))
        .await
        .unwrap();
    assert!(
        messages["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["kind"] == "join_result")
    );
    finish(started);
    runtime.finish();
}

#[tokio::test]
async fn failed_batch_does_not_publish_its_readiness_or_actor_changes() {
    let runtime = RuntimeFixture::new();
    let sequence = runtime.hub.published.observation.lock_or_panic().sequence;
    let result = runtime
        .hub
        .with(|core| {
            core.started("uncommitted".into());
            core.store().test_hooks().at_sql("commit", |_| {
                Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR_WRITE),
                    None,
                ))
            });
            Ok(())
        })
        .await;
    assert_eq!(result.unwrap_err().code(), "database_error");
    assert_eq!(
        runtime.hub.published.observation.lock_or_panic().sequence,
        sequence
    );
    assert_eq!(
        runtime
            .hub
            .published
            .observation
            .lock_or_panic()
            .fields
            .actors,
        0
    );
    assert_eq!(runtime.finish_failed().code(), "database_error");
}

#[tokio::test]
async fn rolled_back_readiness_candidate_is_not_cached_or_published() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut agent = core.store().agent("agent")?;
            agent.state = crate::runtime::domain::AgentState::Idle;
            core.store().save(&agent)
        })
        .await
        .unwrap();
    let started = start(&runtime).await;
    assert_eq!(
        read(&started.reader, json!({"op":"ping"})).await.unwrap()["storage"]["ready"],
        true
    );
    let publication = runtime.hub.published.clone();
    let failed: Result<()> = runtime
        .hub
        .with(move |core| {
            let mut agent = core.store().agent("agent")?;
            agent.paused = true;
            core.store().save(&agent)?;
            assert!(!publication.prepare(core.store())?.ready);
            // A provider/batch read still observes its own uncommitted transaction.
            assert!(!core.store().has_ready()?);
            Err(Error::invalid_request("roll back readiness mutation"))
        })
        .await;
    crate::test_support::assert_code!(failed, "invalid_request");
    assert_eq!(
        read(&started.reader, json!({"op":"ping"})).await.unwrap()["storage"]["ready"],
        true
    );
    // total_changes no longer advances; the read-only batch must not reuse the
    // discarded false candidate from before rollback.
    runtime
        .hub
        .with(|core| {
            assert!(core.store().has_ready()?);
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        read(&started.reader, json!({"op":"ping"})).await.unwrap()["storage"]["ready"],
        true
    );
    finish(started);
    runtime.finish();
}
