use super::*;

#[test]
fn ready_server_passes_the_probe_and_is_stopped_and_reaped() {
    let directory = tempfile::tempdir().unwrap();
    let program = fake_server(directory.path(), &COMMIT[..7]);
    let record = directory.path().join("argv.json");
    let (launch, config) = launch(&program, "ok", Some(&record));
    let used = with_server(&launch, &config, &|| false, || Ok(42)).unwrap();
    assert_eq!(used, 42);
    let argv: Vec<String> = serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    assert!(argv.windows(2).any(|pair| pair == ["--flash-attn", "on"]));
    assert!(argv.windows(2).any(|pair| pair == ["--host", "127.0.0.1"]));
    // The port is free again: the child was stopped and reaped.
    assert!(TcpListener::bind((HOST, launch.port)).is_ok());
}

#[test]
fn wrong_dimensions_fail_the_probe_step() {
    let directory = tempfile::tempdir().unwrap();
    let program = fake_server(directory.path(), &COMMIT[..7]);
    let (launch, config) = launch(&program, "dims", None);
    let error = with_server(&launch, &config, &|| false, || Ok(())).unwrap_err();
    assert_eq!(
        error.message,
        crate::Error::llama_server_probe_failed().message
    );
    assert!(TcpListener::bind((HOST, launch.port)).is_ok());
}

#[test]
fn early_exit_and_never_ready_are_refused_with_private_output() {
    let directory = tempfile::tempdir().unwrap();
    let program = fake_server(directory.path(), &COMMIT[..7]);
    let (exiting, _) = launch(&program, "exit", None);
    let running = exiting.spawn(&|| false).unwrap();
    let error = running
        .wait_ready(Instant::now() + READY_DEADLINE, &|| false)
        .unwrap_err();
    assert_eq!(
        error.message,
        crate::Error::llama_server_exited(Some(3), None).message
    );
    assert!(!error.message.contains("PRIVATE"));
    let cause = std::error::Error::source(&error).unwrap().to_string();
    assert!(cause.contains("PRIVATE loading tail"));
    running.stop().unwrap();
    let (loading, _) = launch(&program, "never_ready", None);
    let running = loading.spawn(&|| false).unwrap();
    let error = running
        .wait_ready(Instant::now() + std::time::Duration::from_secs(2), &|| {
            false
        })
        .unwrap_err();
    assert_eq!(
        error.message,
        crate::Error::llama_server_not_ready().message
    );
    running.stop().unwrap();
    assert!(TcpListener::bind((HOST, loading.port)).is_ok());
}

#[test]
fn cancellation_ends_the_wait_and_drop_reaps_the_child() {
    let directory = tempfile::tempdir().unwrap();
    let program = fake_server(directory.path(), &COMMIT[..7]);
    let (loading, _) = launch(&program, "never_ready", None);
    let running = loading.spawn(&|| false).unwrap();
    let started = Instant::now();
    let error = running
        .wait_ready(Instant::now() + READY_DEADLINE, &|| {
            started.elapsed() > std::time::Duration::from_millis(500)
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::IoError);
    drop(running);
    assert!(TcpListener::bind((HOST, loading.port)).is_ok());
}

#[test]
fn an_occupied_port_is_refused_before_spawning() {
    let directory = tempfile::tempdir().unwrap();
    let program = fake_server(directory.path(), &COMMIT[..7]);
    let (occupied, _) = launch(&program, "ok", None);
    let _holder = TcpListener::bind((HOST, occupied.port)).unwrap();
    let error = occupied.spawn(&|| false).err().unwrap();
    assert_eq!(
        error.message,
        crate::Error::llama_server_port_in_use(occupied.port).message
    );
}

/// The failure a monitor records, within a bounded wait.
fn recorded(availability: &Availability) -> crate::Error {
    let until = Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Err(error) = availability.check() {
            return error;
        }
        assert!(Instant::now() < until, "the monitor recorded no failure");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn a_monitored_server_that_exits_is_reported_and_never_restarted() {
    let directory = tempfile::tempdir().unwrap();
    let program = fake_server(directory.path(), &COMMIT[..7]);
    let (launch, _) = launch(&program, "ok", None);
    let availability = Availability::default();
    let running = launch
        .spawn(&|| false)
        .unwrap()
        .monitor(READY_DEADLINE, availability.clone())
        .unwrap();
    running
        .wait_ready(Instant::now() + READY_DEADLINE, &|| false)
        .unwrap();
    // Let the monitor observe readiness before the server disappears.
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert!(availability.check().is_ok());
    let pid = running.pid().unwrap();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(i32::try_from(pid).unwrap()).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    // Embedding work sees one code and the recovery steps, never a raw
    // connection error; the private output tail stays a cause.
    let error = recorded(&availability);
    assert_eq!(error.code, ErrorCode::EmbeddingsUnavailable);
    assert_eq!(error.message, crate::Error::llama_server_stopped().message);
    assert!(!error.message.contains("PRIVATE"));
    // The monitor reaped the child; nothing started another one.
    assert_eq!(running.pid(), None);
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert!(TcpListener::bind((HOST, launch.port)).is_ok());
    assert_eq!(
        availability.check().unwrap_err().message,
        crate::Error::llama_server_stopped().message
    );
    running.stop().unwrap();
}

#[test]
fn a_monitored_server_that_never_becomes_ready_is_stopped_and_reported() {
    let directory = tempfile::tempdir().unwrap();
    let program = fake_server(directory.path(), &COMMIT[..7]);
    let (loading, _) = launch(&program, "never_ready", None);
    let availability = Availability::default();
    let running = loading
        .spawn(&|| false)
        .unwrap()
        .monitor(std::time::Duration::from_secs(2), availability.clone())
        .unwrap();
    let error = recorded(&availability);
    assert_eq!(error.code, ErrorCode::EmbeddingsUnavailable);
    assert_eq!(error.message, crate::Error::llama_server_stopped().message);
    // The specific step stays the cause, with the private output below it.
    let cause = std::error::Error::source(&error)
        .and_then(|cause| cause.downcast_ref::<crate::Error>())
        .unwrap();
    assert_eq!(
        cause.message,
        crate::Error::llama_server_not_ready().message
    );
    // The monitor stops the loading child instead of leaving it holding memory.
    let until = Instant::now() + std::time::Duration::from_secs(30);
    while running.pid().is_some() {
        assert!(Instant::now() < until, "the failed server was not stopped");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(TcpListener::bind((HOST, loading.port)).is_ok());
    running.stop().unwrap();
}
