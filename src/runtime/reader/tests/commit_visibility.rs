//! Commit visibility regressions.
use super::*;

#[tokio::test]
async fn reads_committed_state_while_writer_is_blocked_and_sees_acknowledged_changes() {
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
    let initial_ping = read(&started.reader, json!({"op":"ping"})).await.unwrap();
    assert_eq!(initial_ping["storage"]["ready"], true);
    let (entered, wait) = oneshot::channel();
    let (release, released) = WriterGate::new();
    let hub = runtime.hub.clone();
    let write = tokio::spawn(async move {
        hub.with(move |core| {
            let mut agent = core.store().agent("agent")?;
            agent.paused = true;
            core.store().save(&agent)?;
            core.started("publication-fixture".into());
            entered.send(()).unwrap();
            released.recv().unwrap();
            Ok(())
        })
        .await
    });
    wait.await.unwrap();
    let prior = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let ping = read(&started.reader, json!({"op":"ping"})).await?;
        let status = read(&started.reader, json!({"op":"status", "target":"agent"})).await?;
        Ok::<_, Error>((ping, status))
    })
    .await;
    // Always unblock the writer before assertions so a failure cannot strand it.
    release.release();
    write.await.unwrap().unwrap();
    let (prior_ping, prior) = prior.unwrap().unwrap();
    assert_eq!(
        prior_ping["writer_observation"]["sequence"],
        initial_ping["writer_observation"]["sequence"]
    );
    assert_eq!(
        prior_ping["resident_actors"],
        initial_ping["resident_actors"]
    );
    let committed_ping = read(&started.reader, json!({"op":"ping"})).await.unwrap();
    assert_eq!(committed_ping["resident_actors"], 1);
    assert_eq!(prior_ping["storage"]["ready"], true);
    assert_eq!(committed_ping["storage"]["ready"], false);
    assert!(
        committed_ping["writer_observation"]["sequence"]
            .as_u64()
            .unwrap()
            > prior_ping["writer_observation"]["sequence"]
                .as_u64()
                .unwrap()
    );
    assert_eq!(
        prior_ping["writer_observation"]["runtime_id"],
        committed_ping["writer_observation"]["runtime_id"]
    );
    assert_eq!(prior["agent"]["paused"], false);
    assert_eq!(
        read(&started.reader, json!({"op":"status", "target":"agent"}))
            .await
            .unwrap()["agent"]["paused"],
        true
    );
    finish(started);
    runtime.finish();
}
