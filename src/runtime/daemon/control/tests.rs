use super::*;
mod cleanup_receipts;

#[tokio::test]
async fn m5b_tree_selector_errors_precede_busy_long_poll_capacity() {
    let runtime = crate::runtime::test_support::RuntimeFixture::new();
    let shutdown = runtime
        .hub
        .with(|core| Ok(core.shutdown().clone()))
        .await
        .unwrap();
    let started = Reader::start(
        crate::runtime::store::Queries::open_read_only(&runtime.temp.path().join("runtime.sqlite"))
            .unwrap(),
        runtime.hub.published.clone(),
        shutdown,
        Arc::default(),
        runtime.hub.wake.clone(),
    );
    let mut permits = Vec::new();
    while let Ok(permit) = runtime.hub.wake.live.admit_if(true) {
        permits.push(permit);
    }
    for selectors in [
        serde_json::json!({"root":7}),
        serde_json::json!({"all":"yes"}),
        serde_json::json!({"root":"a","all":true}),
    ] {
        let mut args = serde_json::json!({"op":"tree", "after":runtime.hub.wake.live.tree_version(), "wait_ms":25000});
        args.as_object_mut()
            .unwrap()
            .extend(selectors.as_object().unwrap().clone());
        let error = tokio::time::timeout(
            Duration::from_secs(1),
            tree(&runtime.hub, &started.reader, RequestEnvelope::new(args)),
        )
        .await
        .unwrap()
        .err()
        .unwrap();
        assert!(matches!(error, Error::Input { .. }));
    }
    drop(permits);
    drop(started.reader);
    started.thread.join().unwrap().unwrap();
    runtime.finish();
}

#[tokio::test]
async fn m5b_daemon_admits_exact_limit_and_rejects_overflow_before_utf8_json() {
    use tokio::io::AsyncWriteExt;
    for case in 0..3 {
        let mut bytes = br#"{"protocol":1,"id":"test","args":{"op":"ping"}}"#.to_vec();
        bytes.resize(1024 * 1024, b' ');
        if case == 0 {
            bytes.pop();
            bytes.push(b'\n');
        } else {
            if case == 2 {
                bytes[0] = 0xff;
            }
            bytes.extend_from_slice(b"hidden\n");
        }
        let (reader, mut writer) = tokio::net::UnixStream::pair().unwrap();
        let (reader, _) = reader.into_split();
        let send = async {
            if let Err(error) = writer.write_all(&bytes).await {
                assert!(matches!(
                    error.kind(),
                    std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                ));
            }
        };
        let (result, ()) = tokio::join!(read_request(reader), send);
        if case == 0 {
            assert!(result.is_ok());
        } else {
            let error = result.err().unwrap();
            assert_eq!(error.code(), "invalid_request");
            assert!(
                serde_json::to_string(&error)
                    .unwrap()
                    .contains("exceeds 1 MiB")
            );
        }
    }
}

#[test]
fn failed_handlers_and_lost_replies_are_visible_bounded_and_private() {
    use crate::{observability, test_support};
    const CHILD: &str = "HIVE_CONTROL_LOG_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let (_, module) = module_path!().split_once("::").unwrap();
        let mut command = test_support::rerun_isolated(
            &format!("{module}::failed_handlers_and_lost_replies_are_visible_bounded_and_private"),
            false,
        );
        test_support::run_isolated(command.env(CHILD, "1"));
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("diagnostics.jsonl");
    let (backend, guard) = hive_log::start(
        &path,
        hive_log::Config {
            level: tracing::level_filters::LevelFilter::DEBUG,
            ..Default::default()
        },
    )
    .unwrap();
    backend.clone().install_global().unwrap();
    backend.install_panic_hook();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(exercise_failed_handlers());
    for _ in 0..1000 {
        let (sender, receiver) = oneshot::channel();
        drop(receiver);
        observability::reply(
            sender.send("private-reply-canary"),
            observability::ReplyOwner::Writer,
        );
    }
    observability::finish();
    assert_eq!(
        guard.finish(Duration::from_secs(2)).state,
        hive_log::Shutdown::Drained
    );
    let text = std::fs::read_to_string(path).unwrap();
    assert!(!text.contains("private-"));
    let rows: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let failures: Vec<_> = rows
        .iter()
        .filter(|r| r["event"] == "control.handler_failed")
        .collect();
    assert_eq!(failures.len(), 3);
    assert_eq!(
        failures.iter().filter(|r| r["code"] == "panicked").count(),
        2
    );
    assert_eq!(
        failures.iter().filter(|r| r["code"] == "cancelled").count(),
        1
    );
    assert!(failures.iter().all(|r| r["decision"] == "retain_siblings"));
    assert!(failures.iter().all(|r| r["task_id"].is_string()));
    assert!(
        rows.iter()
            .any(|r| r["event"] == "worker.panicked" && r["agent_id"] == "worker-fixture")
    );
    let sampled: Vec<_> = rows.iter().filter(|r| r["event"] == "reply.lost").collect();
    assert_eq!(sampled.len(), 10);
    assert_eq!(sampled.last().unwrap()["count"], 512);
    let totals: Vec<_> = rows
        .iter()
        .filter(|r| r["event"] == "reply.loss_total")
        .collect();
    assert_eq!(totals.len(), 1);
    assert_eq!(totals[0]["count"], 1000);
}

async fn exercise_failed_handlers() {
    let mut clients = JoinSet::new();
    let panicked = clients.spawn(async {
        let _boundary = super::super::collection::WorkerBoundary::new("worker-fixture");
        panic!("private-panic-canary");
    });
    let cancelled = clients.spawn(std::future::pending());
    cancelled.abort();
    while !panicked.is_finished() || !cancelled.is_finished() {
        tokio::task::yield_now().await;
    }
    // Opportunistic reaping must report both outcomes without consuming live siblings.
    let (release, released) = oneshot::channel();
    let sibling = clients.spawn(async { released.await.unwrap() });
    assert!(has_client_capacity(&mut clients));
    assert_eq!(clients.len(), 1);
    assert!(!sibling.is_finished());
    clients.spawn(async { panic!("private-shutdown-panic-canary") });
    release.send(()).unwrap();
    let shutdown = AtomicBool::new(false);
    finish(Ok(()), &mut clients, &shutdown, &hub::Wake::default())
        .await
        .unwrap();
    assert!(sibling.is_finished());
    assert!(!shutdown.load(Ordering::Relaxed));
}

#[tokio::test]
async fn control_request_ids_use_shared_byte_and_control_character_bounds() {
    use tokio::io::AsyncWriteExt;
    for (id, accepted) in [
        ("retry 1".into(), true),
        ("é".repeat(64), true),
        ("é".repeat(65), false),
        ("x".repeat(129), false),
        ("private\nidentity".into(), false),
        ("private\u{85}identity".into(), false),
        (String::new(), false),
    ] {
        let (mut writer, reader) = tokio::net::UnixStream::pair().unwrap();
        let line = format!(
            "{}\n",
            serde_json::json!({"protocol":1,"id":id, "args":{"op":"ping"}})
        );
        writer.write_all(line.as_bytes()).await.unwrap();
        let (read, _write) = reader.into_split();
        let result = read_request(read).await;
        assert_eq!(result.is_ok(), accepted);
        if let Err(error) = result {
            assert!(!error.to_string().contains("private"));
        }
    }
}

#[tokio::test]
async fn stop_is_polled_even_when_every_control_slot_is_live() {
    use crate::runtime::test_support::RuntimeFixture;
    let runtime = RuntimeFixture::new();
    let shutdown = runtime
        .hub
        .with(|core| Ok(core.shutdown().clone()))
        .await
        .unwrap();
    let started = crate::runtime::reader::Reader::start(
        crate::runtime::store::Queries::open_read_only(&runtime.temp.path().join("runtime.sqlite"))
            .unwrap(),
        runtime.hub.published.clone(),
        shutdown.clone(),
        Arc::default(),
        runtime.hub.wake.clone(),
    );
    let pool = provider::Pool::new(
        crate::config::Runtime::default(),
        runtime.hub.clone(),
        Arc::<AtomicBool>::default(),
        shutdown,
        vec![],
    );
    let listener = UnixListener::bind(runtime.temp.path().join("control.sock")).unwrap();
    let mut clients = JoinSet::new();
    for _ in 0..CONTROL_CLIENT_LIMIT {
        clients.spawn(std::future::pending());
    }
    let (stop, stopped) = oneshot::channel();
    {
        let mut acceptor = ClientAcceptor {
            shared: &runtime.hub,
            reader: &started.reader,
            pool: &pool,
            clients: &mut clients,
            accept_failures: std::sync::atomic::AtomicU64::new(0),
        };
        let mut accepting = Box::pin(acceptor.run(&listener, stopped));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(accepting.as_mut().poll(&mut context).is_pending());
        stop.send(()).unwrap();
        assert!(accepting.await.is_ok());
    }
    assert_eq!(clients.len(), CONTROL_CLIENT_LIMIT);
    // Acceptance stops without cancelling any already accepted client.
    clients.abort_all();
    while clients.join_next().await.is_some() {}
    drop(pool);
    drop(started.reader);
    started.thread.join().unwrap().unwrap();
    runtime.finish();
}

#[tokio::test]
async fn fatal_accept_result_signals_shutdown_but_keeps_handlers_owned() {
    let shutdown = Arc::<AtomicBool>::default();
    let wake = Arc::<hub::Wake>::default();
    let finished = Arc::<AtomicBool>::default();
    let mut clients = JoinSet::new();
    let (release, wait) = oneshot::channel();
    let completed = finished.clone();
    clients.spawn(async move {
        wait.await.unwrap();
        completed.store(true, Ordering::Relaxed);
    });
    // A normal client panic remains nonfatal to ownership of its siblings.
    clients.spawn(async {
        panic!("ordinary client fixture");
    });
    let notifier = wake.clone();
    let flag = shutdown.clone();
    let observer = tokio::spawn(async move {
        notifier.supervisor.notified().await;
        assert!(flag.load(Ordering::Relaxed));
        release.send(()).unwrap();
    });
    let error = Error::state_io_from(std::io::Error::other("fatal accept fixture"));
    assert!(
        finish(Err(error), &mut clients, &shutdown, &wake)
            .await
            .is_err()
    );
    assert!(finished.load(Ordering::Relaxed));
    assert!(clients.is_empty());
    observer.await.unwrap();
}

#[tokio::test]
async fn control_owner_panic_waits_for_the_existing_supervisor_future() {
    let shutdown = AtomicBool::new(false);
    let uncertain = AtomicBool::new(false);
    let wake = hub::Wake::default();
    let finished = AtomicBool::new(false);
    let (release, wait) = oneshot::channel();
    let mut owner = tokio::spawn(async {
        panic!("control owner fixture");
    });
    let supervisor = async {
        wait.await.unwrap();
        finished.store(true, Ordering::Relaxed);
        Ok(())
    };
    let unblock = async {
        wake.supervisor.notified().await;
        assert!(shutdown.load(Ordering::Relaxed));
        assert!(uncertain.load(Ordering::Relaxed));
        assert!(!finished.load(Ordering::Relaxed));
        release.send(()).unwrap();
    };
    let (outcome, ()) = tokio::join!(
        supervise(supervisor, &mut owner, &shutdown, &uncertain, &wake),
        unblock,
    );
    assert!(outcome.supervisor.is_ok());
    assert!(outcome.control.unwrap().is_err());
    assert!(finished.load(Ordering::Relaxed));
    // The completed handle was consumed by supervise; never await it again.
}

#[tokio::test]
async fn completed_control_requests_do_not_exhaust_live_connection_capacity() {
    let mut clients = tokio::task::JoinSet::new();
    let finished: Vec<_> = (0..CONTROL_CLIENT_LIMIT)
        .map(|_| clients.spawn(async {}))
        .collect();
    while finished.iter().any(|task| !task.is_finished()) {
        tokio::task::yield_now().await;
    }
    assert_eq!(clients.len(), CONTROL_CLIENT_LIMIT);
    assert!(has_client_capacity(&mut clients));
    assert!(clients.is_empty());

    let pending: Vec<_> = (0..CONTROL_CLIENT_LIMIT)
        .map(|_| clients.spawn(std::future::pending()))
        .collect();
    assert!(!has_client_capacity(&mut clients));
    pending[0].abort();
    while !pending[0].is_finished() {
        tokio::task::yield_now().await;
    }
    assert!(has_client_capacity(&mut clients));
    assert_eq!(clients.len(), CONTROL_CLIENT_LIMIT - 1);
    clients.spawn(std::future::pending());
    assert!(!has_client_capacity(&mut clients));
    clients.abort_all();
    while clients.join_next().await.is_some() {}
    assert!(has_client_capacity(&mut clients));
}

#[tokio::test]
async fn malformed_control_identity_has_safe_envelope_context() {
    use tokio::io::AsyncWriteExt;
    for id in [
        None,
        Some(serde_json::json!(null)),
        Some(serde_json::json!({"private-canary":true})),
        Some(serde_json::json!("")),
    ] {
        let (mut writer, reader) = tokio::net::UnixStream::pair().unwrap();
        let mut raw = serde_json::json!({"protocol":1,"args":{"op":"output","id":"artifact"}});
        if let Some(id) = id {
            raw["id"] = id;
        }
        writer
            .write_all(format!("{raw}\n").as_bytes())
            .await
            .unwrap();
        let (read, _write) = reader.into_split();
        let error = read_request(read).await.err().unwrap();
        let wire = api::value(error);
        assert_eq!(wire["code"], "invalid_request_id");
        assert_eq!(wire["field"], "/id");
        assert!(!wire.to_string().contains("private-canary"));
        assert!(wire.get("received").is_none());
        let example: Value = serde_json::from_str(wire["examples"][0].as_str().unwrap()).unwrap();
        assert!(example["args"].is_object());
    }
}

#[tokio::test]
async fn control_protocol_is_required_strict_and_checked_before_dispatch() {
    use tokio::io::AsyncWriteExt;
    for (raw, code) in [
        (
            r#"{"protocol":2,"id":"future","args":{"op":"shutdown"}}"#,
            "incompatible_protocol",
        ),
        (
            r#"{"protocol":0,"id":"future","args":{"op":"shutdown"}}"#,
            "incompatible_protocol",
        ),
        (
            r#"{"id":"missing","args":{"op":"shutdown"}}"#,
            "invalid_request",
        ),
        (
            r#"{"protocol":1,"protocol":1,"id":"duplicate","args":{"op":"shutdown"}}"#,
            "invalid_request",
        ),
        (
            r#"{"protocol":1,"id":"extra","args":{"op":"shutdown"},"extra":true}"#,
            "invalid_request",
        ),
        (
            r#"{"protocol":null,"id":"null","args":{"op":"shutdown"}}"#,
            "invalid_request",
        ),
    ] {
        let (mut writer, reader) = tokio::net::UnixStream::pair().unwrap();
        writer
            .write_all(format!("{raw}\n").as_bytes())
            .await
            .unwrap();
        let (read, _) = reader.into_split();
        assert_eq!(
            read_request(read).await.err().unwrap().code(),
            code,
            "{raw}"
        );
    }
}

#[test]
fn cleanup_failure_keeps_exact_public_bytes_and_its_native_error_chain() {
    use std::error::Error as _;
    let cause = Error::state_io_from(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    let error = purge_cleanup_error(cause);
    assert_eq!(
        serde_json::to_string(&error).unwrap(),
        concat!(
            "{\"code\":\"purge_cleanup_pending\",\"message\":\"Agent records were purged but output ",
            "or native-session cleanup is incomplete. Retry with the returned request_id.\"}"
        )
    );
    let native = error.source().unwrap().downcast_ref::<Error>().unwrap();
    assert_eq!(native.code(), "state_io");
    assert_eq!(
        native
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
}
