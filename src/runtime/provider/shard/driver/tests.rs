use crate::runtime::{
    provider::{RpcHarness, catalogs::BinaryIdentity, shard::corpus_support::with_driver},
    test_support::RuntimeFixture,
};

#[test]
fn stop_arriving_at_wake_drain_never_enters_an_idle_wait() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let fixture = RuntimeFixture::new();
    let mut rpc = RpcHarness::active(fixture.launcher.clone());
    let waited = Arc::new(AtomicBool::new(false));
    let observed = waited.clone();
    with_driver(&fixture, &mut rpc, |driver, _| {
        driver.shard.occupancy().slots.insert("agent".into());
        driver.hooks.before_drain = Some(Box::new(|shard| {
            shard.request_stop();
            shard.wake_owner();
        }));
        driver.hooks.before_wait = Some(Box::new(move |shard| {
            observed.store(true, Ordering::Release);
            shard.wake_owner(); // Rescue the old ordering before its hour-long poll.
        }));
        driver.event_loop(std::time::Instant::now()).unwrap();
        assert!(driver.shard.stop());
    });
    fixture.finish();
    assert!(!waited.load(Ordering::Acquire));
}

#[test]
fn unrelated_invalidation_during_bootstrap_keeps_its_catalog_and_session_ready() {
    let fixture = RuntimeFixture::new();
    let mut rpc = RpcHarness::active(fixture.launcher.clone());
    with_driver(&fixture, &mut rpc, |driver, output| {
        driver
            .shard
            .starting
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(driver.shard.starting());
        let epoch = driver.pool.hub.catalogs.epoch();
        let binary = BinaryIdentity::read(&std::env::current_exe().unwrap()).unwrap();
        driver.shard.binary_identity.set(binary).unwrap();
        driver.pool.hub.catalogs.invalidate(uuid::Uuid::new_v4());
        let models = vec![super::super::Model::parse(
            &serde_json::json!({"model":"live-model"}),
        )];
        assert!(driver.publish_catalog(epoch, None, Ok(models)).is_none());
        assert_eq!(
            driver.shard.catalog.get().unwrap().as_ref().unwrap()[0].name(),
            Some("live-model")
        );
        assert!(!driver.shard.starting());
        assert!(driver.routes.contains_key("agent"));
        assert!(output.failure.borrow().is_none());
        assert!(!driver.rpc.uncertain());
    });
    fixture.finish();
}
