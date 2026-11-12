use super::*;
use crate::runtime::{provider::WorkerTransport, test_support::RuntimeFixture};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn poisoned_registry_cannot_strand_admission_or_idle_waits_or_spin_interruption() {
    let fixture = RuntimeFixture::new();
    let (agent, shutdown) = fixture
        .hub
        .with(|core| Ok((core.store().agent("agent")?, core.shutdown().clone())))
        .await
        .unwrap();
    let mut transport = WorkerTransport::new(&fixture, serde_json::json!({}));
    let incoming = transport.hold_incoming();
    let turns = Arc::new(Semaphore::new(1));
    let _held = turns.clone().acquire_owned().await.unwrap();
    let notify = Notify::new();
    let mut permit = None;
    {
        let mut driver = SessionLoop {
            applied_turn_selection: agent.desired_selection(),
            selection: agent.desired_selection(),
            relaunch: false,
            execution: ExecutionHandle::new(&fixture.hub, "agent", "execution"),
            state: SessionState::from(&agent),
            rpc: &mut transport.session,
            shutdown: &shutdown,
            notify: &notify,
            turns: &turns,
            permit: &mut permit,
            idle_seconds: 0,
            thread_id: "thread",
            idle_at: Instant::now(),
            stop_deadline: None,
            interrupt_sent: false,
            admission: None,
            active: None,
            pending_steering: vec![],
            #[cfg(feature = "comb")]
            pending_comb: vec![],
        };
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                fixture.hub.wake.poison_agents_for_test();
            }))
            .is_err()
        );
        shutdown.store(true, Ordering::Release);
        fixture.hub.wake.all();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), driver.await_admission())
                .await
                .unwrap()
                .unwrap()
        );
        tokio::time::timeout(Duration::from_millis(100), driver.wait_for_event(false))
            .await
            .unwrap()
            .unwrap();
        driver.active = Some(ActiveTurn {
            message: Message {
                has_attachments: false,
                attachments: vec![],
                joined: false,
                id: "message".into(),
                sender: "human".into(),
                recipient: "agent".into(),
                body: "body".into(),
                state: MessageState::Submitting,
            },
            turn_id: "turn".into(),
            started: Instant::now(),
        });
        driver.stop_deadline = Some(Instant::now() + Duration::from_secs(10));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), driver.wait_for_event(true))
                .await
                .is_err()
        );
    }
    assert!(permit.is_none());
    drop(incoming);
    transport.finish().await;
    fixture.finish();
}
