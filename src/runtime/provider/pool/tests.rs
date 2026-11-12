use super::*;
use crate::runtime::{provider::shard::corpus_support, test_support::RuntimeFixture};

#[tokio::test]
async fn shutdown_signals_all_owners_before_waiting_for_either_exit() {
    let fixture = RuntimeFixture::new();
    let pool = fixture.pool();
    let first = corpus_support::shard(&fixture);
    let second = corpus_support::shard(&fixture);
    pool.registry.lock_or_panic().shards = vec![first.shard.clone(), second.shard.clone()];
    let shutdown_pool = pool.clone();
    let stopping = tokio::spawn(async move { shutdown_pool.shutdown().await });
    let both = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !first.shard.stop() || !second.shard.stop() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_ok();
    // Release every fake owner even when the assertion would fail on serial shutdown.
    let released = first.shard.capacity_released() || second.shard.capacity_released();
    for shard in [&first.shard, &second.shard] {
        shard.mark_exited();
        shard.done.notify_waiters();
    }
    stopping.await.unwrap();
    drop((pool, first, second));
    fixture.finish();
    assert!(!released);
    assert!(
        both,
        "shutdown awaited one owner before signalling its sibling"
    );
}

#[tokio::test]
async fn cancelled_attach_retires_empty_owner_but_keeps_its_charged_slot() {
    for peer in [false, true] {
        let fixture = RuntimeFixture::new();
        let pool = fixture.pool();
        let corpus_support::ShardSetup { shard, mut queues } = corpus_support::shard(&fixture);
        shard.occupancy().slots.insert("agent".into());
        shard.occupancy().users.insert("agent".into());
        if peer {
            shard.occupancy().users.insert("peer".into());
        }
        let mut future = Box::pin(pool.attach(shard.clone(), "agent"));
        let command = tokio::select! {
            _ = &mut future => panic!("Attach unexpectedly finished"),
            command = queues.normal.recv() => command,
        };
        assert!(matches!(command, Some(Cmd::Attach(..))));
        drop(future);
        assert_eq!(shard.occupancy().users.is_empty(), !peer);
        assert!(shard.occupancy().slots.contains("agent"));
        assert_eq!(shard.draining(), !peer);
        assert_eq!(shard.abandoned_attach(), !peer);
        assert!(!shard.stop());
        drop(command);
        drop(pool);
        drop(shard);
        drop(queues);
        fixture.finish();
    }
}
