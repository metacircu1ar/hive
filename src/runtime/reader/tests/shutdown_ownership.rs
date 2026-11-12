//! Shutdown ownership regressions.
use super::*;

#[tokio::test]
async fn shutdown_rejects_reads_and_joins_reader_before_writer() {
    let runtime = RuntimeFixture::new();
    let started = start(&runtime).await;
    runtime
        .hub
        .with(|core| {
            core.shutdown().store(true, Ordering::Relaxed);
            Ok(())
        })
        .await
        .unwrap();
    crate::test_support::assert_code!(
        read(&started.reader, json!({"op":"ping"})).await,
        "daemon_stopping"
    );
    finish(started);
    runtime.finish();
}

#[tokio::test]
async fn reader_panic_signals_shutdown_without_cancelling_inflight_writer() {
    let runtime = RuntimeFixture::new();
    let shutdown = runtime
        .hub
        .with(|core| Ok(core.shutdown().clone()))
        .await
        .unwrap();
    let uncertain = Arc::new(AtomicBool::new(false));
    let notifier = Arc::<Wake>::default();
    let started = Reader::start(
        Queries::open_read_only(&runtime.temp.path().join("runtime.sqlite")).unwrap(),
        runtime.hub.published.clone(),
        shutdown.clone(),
        uncertain.clone(),
        notifier.clone(),
    );
    let (entered, wait) = oneshot::channel();
    let (release, released) = WriterGate::new();
    let hub = runtime.hub.clone();
    let write = tokio::spawn(async move {
        hub.with(move |core| {
            entered.send(()).unwrap();
            released.recv().unwrap();
            let mut agent = core.store().agent("agent")?;
            agent.paused = true;
            core.store().save(&agent)
        })
        .await
    });
    wait.await.unwrap();
    let wake = notifier.supervisor.notified();
    tokio::pin!(wake);
    wake.as_mut().enable();
    let (reply, failed) = oneshot::channel();
    started
        .reader
        .sender
        .send(Job::Control(Box::new(ControlJob {
            test_hooks: super::super::super::test_support::TestHooks::before_job(|| {
                panic!("injected reader owner panic");
            }),
            args: api::RequestEnvelope::new(json!({"op":"ping"})),
            owner: None,
            reply,
        })))
        .await
        .unwrap();
    let failure = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let closed = failed.await.is_err();
        wake.await;
        (
            closed,
            shutdown.load(Ordering::Relaxed),
            uncertain.load(Ordering::Relaxed),
        )
    })
    .await;
    release.release();
    write.await.unwrap().unwrap();
    assert_eq!(failure.unwrap(), (true, true, true));
    assert!(
        runtime
            .hub
            .with(|core| Ok(core.store().agent("agent")?.paused))
            .await
            .unwrap()
    );
    drop(started.reader);
    assert!(started.thread.join().is_err());
    runtime.finish();
}

#[tokio::test]
async fn snapshot_cleanup_failure_returns_primary_and_stops_reader_owner() {
    let runtime = RuntimeFixture::new();
    let queries = Queries::open_read_only(&runtime.temp.path().join("runtime.sqlite")).unwrap();
    queries
        .test_hooks()
        .at_sql("snapshot_close", |db| db.execute_batch("ROLLBACK"));
    let shutdown = runtime
        .hub
        .with(|c| Ok(c.shutdown().clone()))
        .await
        .unwrap();
    let started = Reader::start(
        queries,
        runtime.hub.published.clone(),
        shutdown.clone(),
        Arc::default(),
        runtime.hub.wake.clone(),
    );
    let result = read(&started.reader, json!({"op":"status", "target":"missing"})).await;
    assert_eq!(result.unwrap_err().code(), "unknown_agent");
    drop(started.reader);
    assert_eq!(
        started.thread.join().unwrap().unwrap_err().code(),
        "unknown_agent"
    );
    assert!(shutdown.load(Ordering::Relaxed));
    assert_eq!(runtime.hub.wake.failure().unwrap().code(), "unknown_agent");
    runtime.finish();
}
