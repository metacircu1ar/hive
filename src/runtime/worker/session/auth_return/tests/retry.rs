use super::*;
use crate::runtime::{
    provider::WorkerTransport,
    worker::session::{SessionLoop, SessionState},
};
use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
use tokio::sync::{Notify, Semaphore};

#[tokio::test]
async fn exhausted_snapshot_retries_release_the_turn_permit_and_wake_the_same_session() {
    let fixture = RuntimeFixture::new();
    let agent = fixture
        .hub
        .with(|core| {
            let original = core.store().agent("agent")?;
            let agent = setup(core, original);
            core.store().test_hooks().at_sql("begin", |db| {
                db.execute("UPDATE login_availability SET revision=revision+1", [])?;
                Ok(())
            });
            Ok(agent)
        })
        .await
        .unwrap();
    let mut transport = WorkerTransport::new(&fixture, serde_json::json!({}));
    let turns = std::sync::Arc::new(Semaphore::new(1));
    let notify = Notify::new();
    let shutdown = AtomicBool::new(false);
    let mut permit = Some(turns.clone().acquire_owned().await.unwrap());
    let mut driver = SessionLoop {
        applied_turn_selection: agent.boundary_selection(),
        selection: agent.boundary_selection(),
        relaunch: false,
        execution: ExecutionHandle::new(&fixture.hub, "agent", "execution"),
        state: SessionState::from(&agent),
        rpc: &mut transport.session,
        shutdown: &shutdown,
        notify: &notify,
        turns: &turns,
        permit: &mut permit,
        idle_seconds: 60,
        thread_id: "thread",
        idle_at: Instant::now(),
        stop_deadline: None,
        interrupt_sent: false,
        admission: None,
        active: None,
        pending_steering: Vec::new(),
        #[cfg(feature = "comb")]
        pending_comb: Vec::new(),
    };
    driver.start_message(0).await.unwrap();
    assert!(driver.permit.is_none());
    assert!(!driver.relaunch);
    assert_eq!(turns.available_permits(), 1);
    // This Notify is private to this driver; writer notifications cannot satisfy it.
    tokio::time::timeout(Duration::from_millis(100), notify.notified())
        .await
        .unwrap();
    drop(driver);
    assert!(transport.sent.lock().unwrap().is_empty());
    fixture
        .hub
        .with(|core| {
            core.store().test_hooks().at_sql("begin", |_| Ok(()));
            Ok(())
        })
        .await
        .unwrap();
    let admission = claim(
        ExecutionHandle::new(&fixture.hub, "agent", "execution"),
        agent.boundary_selection(),
        None,
    )
    .await
    .unwrap();
    assert!(matches!(admission, Some(TurnAdmission::Submit { .. })));
    transport.finish().await;
    fixture.finish();
}
