use super::corpus_support::{ShardSetup, with_driver};
use crate::runtime::{provider::RpcHarness, test_support::RuntimeFixture};
use serde_json::json;
use std::sync::atomic::Ordering;
#[test]
fn clean_linger_expiry_does_not_skip_unclean_close_grace() {
    let launcher = crate::launch::test_cache().unwrap();
    let fixture = RuntimeFixture::new();
    let mut rpc = RpcHarness::new(launcher.clone());
    with_driver(&fixture, &mut rpc, |driver, _| {
        driver.shard.occupancy().slots.insert("agent".into());
        driver.shard.begin_drain();
        // Empty users alone is not evidence of a clean retirement.
        assert!(!driver.expire_spare());
        driver.shard.reset_drain_for_test();
        driver.shard.occupancy().idle_since = Some(std::time::Instant::now());
        assert!(!driver.expire_spare());
        driver.shard.occupancy().idle_since = Some(
            std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(10))
                .unwrap(),
        );
        assert!(driver.expire_spare());
        assert!(driver.shard.draining());
    });
    fixture.finish();
}

#[test]
fn claude_identity_recycle_fences_the_shard_before_queued_input_can_follow_completion() {
    use super::*;
    use crate::runtime::provider::claude::{SessionConfig, tests::Fixture};
    let launcher = crate::launch::test_cache().unwrap();
    let fixture = RuntimeFixture::new();
    let mut native = Fixture::with_id(None, "thread".into());
    let turn = native.turn();
    for n in 0..2100 {
        native.codec.receive(json!({"type":"stream_event", "session_id":"thread", "parent_tool_use_id":null,
            "event":{"type":"content_block_start", "content_block":{"type":"tool_use", "id":format!("stream-{n}"), "name":"Read", "input":{}}}})).unwrap();
    }
    native
        .codec
        .receive(
            json!({"type":"result", "session_id":"thread", "subtype":"success", "is_error":false,
        "terminal_reason":"completed", "user_message_uuid":turn}),
        )
        .unwrap();
    let completed = IncomingFrame::parse(native.codec.pop().unwrap());
    let backend = super::super::Backend::Claude(Arc::new(SessionConfig::for_test(
        "thread",
        fixture.temp.path().into(),
    )));
    let mut rpc = RpcHarness::new(launcher);
    rpc.native_codec(native.codec);
    corpus_support::with_backend(&fixture, &mut rpc, backend, |driver, output| {
        let (tx, mut rx) = oneshot::channel();
        driver
            .shard
            .normal
            .try_send(Cmd::Call(
                "agent".into(),
                outgoing::turn_input("thread", "next"),
                tx,
            ))
            .unwrap_or_else(|_| panic!("queue next input"));
        driver
            .rpc
            .pending
            .push_back(completed.clone(), completed.encoded_len());
        driver.drain_frames().unwrap();
        assert!(driver.shard.draining());
        assert_eq!(
            output.received().unwrap()["params"]["turn"]["status"],
            "completed"
        );
        driver.drain_commands(&mut Instant::now()).unwrap();
        let Err(error) = rx.try_recv().unwrap() else {
            panic!("a draining provider must reject the next input");
        };
        assert_eq!(error.code(), "provider_draining");
        assert!(driver.pending.is_empty());
    });
    fixture.finish();
}

#[test]
fn confirmed_claude_exit_drains_observations_past_cancelled_callbacks_only() {
    use super::{corpus_support::shard, *};
    use crate::runtime::provider::session::Session;
    for (confirmed, failed) in [(true, false), (false, false), (true, true)] {
        let fixture = RuntimeFixture::new();
        let ShardSetup { shard, queues } = shard(&fixture);
        shard.publish_final_events(confirmed);
        let corpus_support::RouteSetup {
            route,
            output:
                corpus_support::RouteOutput {
                    events,
                    failure: failed_rx,
                },
        } = Route::for_test(8, Arc::default());
        let total = Arc::new(AtomicUsize::new(0));
        for raw in [
            json!({"id":1,"method":"item/tool/call","params":{"threadId":"thread","turnId":"turn"}}),
            json!({"method":"item/completed","params":{"threadId":"thread","turnId":"turn"}}),
            json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"turn","status":"failed"}}}),
        ] {
            route.send(IncomingFrame::parse(raw), &total).unwrap();
        }
        if failed {
            route.failure.send(Some(protocol())).unwrap();
        }
        drop(route);
        let mut session = Session {
            pool: fixture.pool(),
            shard,
            agent: "agent".into(),
            events,
            failure: failed_rx,
            started: Started {
                pid: 0,
                before: 0,
                after: 0,
            },
            thread: Some("thread".into()),
            released: true,
        };
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                if confirmed && !failed {
                    assert_eq!(
                        session.recv().await.unwrap().method().unwrap().as_str(),
                        "item/completed"
                    );
                    assert_eq!(
                        session.recv().await.unwrap().method().unwrap().as_str(),
                        "turn/completed"
                    );
                }
                assert!(session.recv().await.is_err());
            });
        drop(session);
        drop(queues);
        fixture.finish();
    }
}

#[test]
fn codex_shutdown_fences_queued_fork_without_native_creation() {
    use super::*;
    let launcher = crate::launch::test_cache().unwrap();
    let fixture = RuntimeFixture::new();
    let mut rpc = RpcHarness::new(launcher.clone());
    corpus_support::with_driver(&fixture, &mut rpc, |driver, _| {
        let (tx, mut rx) = oneshot::channel();
        driver
            .shard
            .normal
            .try_send(Cmd::Call(
                "agent".into(),
                outgoing::thread_fork(None, "source", "cut"),
                tx,
            ))
            .unwrap_or_else(|_| panic!("queue fork"));
        driver.fence_commands();
        assert!(
            rx.try_recv()
                .unwrap()
                .err()
                .unwrap()
                .input_was_not_submitted()
        );
        assert!(driver.pending.is_empty());
    });
    fixture.finish();
}

#[test]
fn claude_shutdown_fences_and_rejects_already_queued_input_as_unsent() {
    use super::*;
    use crate::runtime::provider::{Backend, claude::SessionConfig};
    let launcher = crate::launch::test_cache().unwrap();
    let fixture = RuntimeFixture::new();
    let mut rpc = RpcHarness::new(launcher.clone());
    let backend = Backend::Claude(Arc::new(SessionConfig {
        history: crate::runtime::provider::claude::History::Resume(
            crate::runtime::provider::claude::IdentityPrompt::NativeSnapshot,
        ),
        instructions: String::new(),
        ..SessionConfig::for_test("reserved", fixture.temp.path().into())
    }));
    corpus_support::with_backend(&fixture, &mut rpc, backend, |driver, _| {
        let (tx, mut rx) = oneshot::channel();
        driver
            .shard
            .normal
            .try_send(Cmd::Call(
                "agent".into(),
                outgoing::turn_input("thread", ""),
                tx,
            ))
            .unwrap_or_else(|_| panic!("queue input"));
        driver.fence_commands();
        assert!(driver.shard.draining());
        assert!(
            rx.try_recv()
                .unwrap()
                .err()
                .unwrap()
                .input_was_not_submitted()
        );
        assert!(driver.normal.is_closed());
        assert!(driver.priority.is_closed());
        assert!(driver.essential.is_closed());
    });
    fixture.finish();
}

#[test]
fn release_after_command_fence_waits_for_final_shutdown_receipt() {
    use super::*;
    use crate::runtime::provider::session::Session;
    let fixture = RuntimeFixture::new();
    let ShardSetup { shard, mut queues } = corpus_support::shard(&fixture);
    queues.normal.close();
    queues.priority.close();
    queues.essential.close();
    let (_tx, events) = mpsc::channel(1);
    let (_failure, failed_rx) = watch::channel(None);
    let mut session = Session {
        pool: fixture.pool(),
        shard: shard.clone(),
        agent: "agent".into(),
        events,
        failure: failed_rx,
        started: Started {
            pid: 0,
            before: 0,
            after: 0,
        },
        thread: None,
        released: false,
    };
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(async {
            let close = tokio::spawn(async move { session.close(None).await });
            tokio::task::yield_now().await;
            assert!(!close.is_finished());
            *shard.report.lock_or_panic() = Some(Shutdown {
                method: crate::runtime::provider::ShutdownMethod::Eof,
                confirmed: true,
                quiescent: true,
                reaped: true,
                exit_code: Some(0),
            });
            shard.mark_exited();
            shard.done.notify_waiters();
            assert!(close.await.unwrap().confirmed);
        });
    drop(queues);
    drop(shard);
    fixture.finish();
}

#[test]
fn retirement_releases_held_turn_permit_after_cleanup_before_unload() {
    use super::*;
    use crate::runtime::provider::session::Session;
    use tokio::sync::Semaphore;

    let fixture = RuntimeFixture::new();
    let ShardSetup { shard, mut queues } = corpus_support::shard(&fixture);
    let (event_sender, events) = mpsc::channel(1);
    let (_failure, failed_rx) = watch::channel(None);
    let mut session = Session {
        pool: fixture.pool(),
        shard,
        agent: "agent".into(),
        events,
        failure: failed_rx,
        started: Started {
            pid: 0,
            before: 0,
            after: 0,
        },
        thread: Some("thread".into()),
        released: false,
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            let semaphore = Arc::new(Semaphore::new(1));
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let close = tokio::spawn(async move { session.close(Some(permit)).await });
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                exercise_retirement(&mut queues, &semaphore, &close),
            )
            .await
            .expect("retirement deadline");
            assert!(result);
            let result = close.await.unwrap();
            assert!(result.confirmed);
        });
    drop(event_sender);
    drop(queues);
    fixture.finish();
}

async fn exercise_retirement(
    queues: &mut super::init::ShardChannels,
    semaphore: &tokio::sync::Semaphore,
    close: &tokio::task::JoinHandle<super::Shutdown>,
) -> bool {
    use super::*;
    let mut inventory = None;
    while inventory.is_none() {
        let command = tokio::select! {
            command = queues.priority.recv() => command,
            command = queues.essential.recv() => command,
        }
        .expect("command channel");
        match command {
            Cmd::Inspect(_, reply) => reply.send(Ok(None)).unwrap(),
            Cmd::Call(_, request, reply) => match request.method().as_str() {
                "thread/backgroundTerminals/clean" => {
                    assert_eq!(semaphore.available_permits(), 0);
                    assert!(reply.send(Ok(Response::parse(json!({})))).is_ok());
                }
                "thread/backgroundTerminals/list" => {
                    assert_eq!(semaphore.available_permits(), 0);
                    assert!(
                        reply
                            .send(Ok(Response::parse(json!({"data":[],"nextCursor":null}))))
                            .is_ok()
                    );
                }
                "thread/unsubscribe" => {
                    assert_eq!(semaphore.available_permits(), 1);
                    assert!(
                        reply
                            .send(Ok(Response::parse(json!({"status":"unsubscribed"}))))
                            .is_ok()
                    );
                }
                "thread/loaded/list" => inventory = Some(reply),
                method => panic!("unexpected request {method}"),
            },
            Cmd::PrepareRetirement(_, reply) => reply.send(Ok(())).unwrap(),
            _ => panic!("unexpected retirement command"),
        }
    }
    assert_eq!(semaphore.available_permits(), 1);
    assert!(!close.is_finished());
    assert!(
        inventory
            .unwrap()
            .send(Ok(Response::parse(json!({"data":[],"nextCursor":null}))))
            .is_ok()
    );
    loop {
        match queues.essential.recv().await.expect("command channel") {
            Cmd::Retire(_, reply) => reply.send(Ok(())).unwrap(),
            Cmd::Release { reply, .. } => {
                return reply
                    .send(Shutdown {
                        method: crate::runtime::provider::ShutdownMethod::Eof,
                        confirmed: true,
                        quiescent: true,
                        reaped: true,
                        exit_code: Some(0),
                    })
                    .is_ok();
            }
            _ => panic!("unexpected release command"),
        }
    }
}

#[test]
fn pool_snapshot_counts_live_recovered_and_released_capacity_separately() {
    use crate::runtime::locks::MutexExt;
    let fixture = RuntimeFixture::new();
    let pool = fixture.pool();
    let setups: Vec<_> = (0..6)
        .map(|_| super::corpus_support::shard(&fixture))
        .collect();
    let shards: Vec<_> = setups.iter().map(|setup| setup.shard.clone()).collect();
    shards[1].starting.store(true, Ordering::Release);
    shards[2].occupancy().slots.extend(["a".into(), "b".into()]);
    shards[2].occupancy().idle_since = Some(std::time::Instant::now());
    shards[3].occupancy().slots.insert("c".into());
    shards[3].occupancy().users.insert("c".into());
    shards[3].begin_drain();
    shards[4]
        .occupancy()
        .slots
        .extend(["d".into(), "e".into(), "f".into()]);
    shards[4].mark_exited();
    shards[4].begin_drain();
    shards[5].occupancy().slots.insert("excluded".into());
    shards[5].release_capacity();
    {
        let mut registry = pool.registry.lock_or_panic();
        registry.shards = shards;
        registry.probes = 2;
        registry
            .recovered
            .push(super::super::registry::RecoveredReservation {
                process_options: None,
                reservation: crate::state::RecoveryReservation {
                    auth: "main".into(),
                    provider: "recovered".into(),
                    process: Some(crate::state::ProcessIdentity::current().unwrap()),
                    gone: false,
                    unknown_capacity: false,
                    unresolved_activity: true,
                },
                slots: 4,
            });
    }
    assert_eq!(
        crate::api::value(pool.snapshot()),
        json!({
            "reserved_shards":8, "recovered_provider_reservations":1,
            "unreaped_provider_reservations":1, "recovered_unresolved_activity_profiles":1,
            "resident_session_slots":10, "attached_agents":1, "starting_shards":3,
            "active_turns":0,
            "warm_shards":1, "lingering_shards":1, "draining_shards":2,
            "provider_versions":[],
            "limits":pool.settings,
        })
    );
    drop(pool);
    drop(setups);
    fixture.finish();
}
