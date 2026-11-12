use super::*;

mod bootstrap;

fn draining_shards(f: &Harness) -> usize {
    #[derive(serde::Deserialize)]
    struct Ping {
        runtime_resources: Resources,
    }
    #[derive(serde::Deserialize)]
    struct Resources {
        draining_shards: usize,
    }
    let ping: Ping = serde_json::from_value(f.ok(json!({"op":"ping"}))).unwrap();
    ping.runtime_resources.draining_shards
}

#[test]
fn same_boot_restart_allows_fresh_agents_but_never_replays_uncertain_input() {
    let mut f = Harness::new();
    let pid_file = f.tmp.path().join("survivor.pid");
    let stop_file = f.tmp.path().join("survivor.stop");
    let mut survivor = SurvivorRelease {
        stop: stop_file.clone(),
        pid: pid_file.clone(),
        identity: None,
    };
    f.set_scenario(
        Provider::Codex,
        json!({"linger_eof":true,"pid_file":pid_file,"eof_stop_file":stop_file}),
    );
    f.start();
    let id = f.spawn("hold");
    let before = f.wait(&id, |a| a.state == "running");
    let pid = fs::read_to_string(&pid_file).unwrap().parse().unwrap();
    let original = hive::state::ProcessIdentity::read(pid).unwrap().unwrap();
    survivor.identity = Some(original.clone());
    let mut daemon = f.daemon.take().unwrap();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    let state = State::new(f.root.clone()).unwrap();
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let binding = state.lookup("main", &profile).unwrap().unwrap();
    assert_eq!(
        state
            .operation(&binding, hive::state::OperationKind::Login)
            .err()
            .unwrap()
            .code(),
        "profile_busy"
    );
    let report = serde_json::to_value(state.recovery_report(&binding).unwrap()).unwrap();
    assert_eq!(report["runtime_usable"], true);
    f.set_scenario(Provider::Codex, json!({}));
    f.start();
    let next = f.spawn("complete");
    f.wait(&next, |a| {
        a.task_outcome == "completed" && a.state == "stopped"
    });
    assert!(!original.absent().unwrap());
    assert_eq!(
        state
            .operation(&binding, hive::state::OperationKind::Login)
            .err()
            .unwrap()
            .code(),
        "profile_busy"
    );
    fs::write(stop_file, "stop").unwrap();
    assert_eq!(
        support::reply::Failure::parse(f.call(json!({"op":"resume","target":id})))
            .error
            .code,
        "recovery_required"
    );
    let old = f.status_reply(json!({"op":"status","target":id}));
    assert_eq!(old.agent.thread_id, before.thread_id);
    let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
    let delivery: String = db
        .query_row(
            "SELECT state FROM messages WHERE recipient=?1 ORDER BY seq LIMIT 1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(delivery, "uncertain");
    f.shutdown();
    assert_eq!(
        serde_json::to_value(state.recovery_report(&binding).unwrap()).unwrap()["runtime_usable"],
        true
    );
}
#[test]
fn failed_provider_does_not_stop_same_profile_siblings_or_fresh_work() {
    let mut f = Harness::new();
    configure_runtime(
        &f,
        json!({"sessions_per_codex_process":1,"max_providers":4}),
        false,
    );
    f.start();
    let sibling = f.spawn("hold");
    let saved = f.wait(&sibling, |a| a.state == "running");
    f.set_scenario(Provider::Codex, json!({"cleanup_refused":true}));
    let bad = f.spawn("idle");
    f.settled(&bad);
    f.ok(json!({"op":"stop","target":bad}));
    f.wait(&bad, |a| a.state == "failed");
    let active = f.status_reply(json!({"op":"status","target":sibling}));
    assert_eq!(active.agent.state, "running");
    assert_eq!(active.agent.provider_id, saved.provider_id);
    f.set_scenario(Provider::Codex, json!({}));
    let fresh = f.spawn("complete");
    f.wait(&fresh, |a| {
        a.task_outcome == "completed" && a.state == "stopped"
    });
    assert_eq!(
        support::reply::Failure::parse(f.call(json!({"op":"resume","target":bad})))
            .error
            .code,
        "recovery_required"
    );
    f.ok(json!({"op":"interrupt","target":sibling}));
    f.wait(&sibling, |a| a.state == "idle");
    f.ok(json!({"op":"send","target":sibling,"body":"complete"}));
    f.wait(&sibling, |a| {
        a.task_outcome == "completed" && a.state == "stopped"
    });
    f.shutdown();
    f.assert_recovery_required();
}
#[test]
fn shared_clean_conversation_resumes_while_another_session_keeps_provider_alive() {
    let mut f = Harness::new();
    configure_runtime(
        &f,
        json!({"sessions_per_codex_process":4,"max_providers":1,"idle_seconds":600}),
        false,
    );
    f.start();
    let sibling = f.spawn("idle");
    f.settled(&sibling);
    let clean = f.spawn("idle");
    let before = f.settled(&clean);
    f.ok(json!({"op":"stop","target":clean}));
    let stopped = f.wait(&clean, |a| a.state == "stopped");
    assert!(stopped.conversation_clean);
    assert!(f.call(json!({"op":"resume","target":clean}))["error"].is_null());
    f.ok(json!({"op":"send","target":clean,"body":"complete"}));
    let after = f.wait(&clean, |a| {
        a.state == "stopped" && a.task_outcome == "completed"
    });
    assert_eq!(before.thread_id, after.thread_id);
    assert_eq!(before.provider_id, after.provider_id);
    f.ok(json!({"op":"terminate","target":sibling}));
    f.wait(&sibling, |a| a.state == "stopped");
    f.shutdown();
}

#[test]
fn delayed_native_unload_preserves_sibling_progress_and_resume_ownership() {
    let mut f = Harness::builder(Provider::Codex)
        .runtime(json!({"sessions_per_codex_process":4,"max_providers":1,
            "max_active_turns":1,"idle_seconds":600}))
        .scenario(json!({"unload_after_unsubscribe_ms":12000}))
        .build();
    f.start();
    let sibling = f.spawn("idle");
    let sibling_before = f.settled(&sibling);
    let agent = f.spawn("hold");
    let before = f.wait(&agent, |a| a.state == "running");
    let thread = before.thread_id.as_deref().unwrap();
    assert_eq!(before.provider_id, sibling_before.provider_id);
    f.ok(json!({"op":"stop","target":agent}));
    support::wait::wait_until(
        Duration::from_secs(10),
        Duration::from_millis(20),
        "unsubscribe",
        || {
            f.provider_home
                .join(format!("unsubscribed-{thread}"))
                .exists()
                .then_some(())
        },
    );
    assert_ne!(f.status(&agent).state, "stopped");
    assert!(!f.status(&agent).conversation_clean);
    let sent = f.ok(json!({"op":"send","target":sibling,"body":"idle"}));
    let message = sent["message_id"].as_str().unwrap().to_owned();
    let advanced = f.wait(&sibling, |a| {
        a.last_finished_turn
            .as_ref()
            .and_then(|turn| turn.message.as_deref())
            == Some(message.as_str())
    });
    assert_eq!(advanced.provider_id, sibling_before.provider_id);
    assert!(!f.provider_home.join(format!("unloaded-{thread}")).exists());
    let stopped = f.wait_timeout(&agent, Duration::from_secs(30), |a| a.state == "stopped");
    assert!(f.provider_home.join(format!("unloaded-{thread}")).exists());
    assert!(stopped.conversation_clean);
    assert_eq!(draining_shards(&f), 0);
    f.ok(json!({"op":"resume","target":agent}));
    f.ok(json!({"op":"send","target":agent,"body":"idle"}));
    let resumed = f.settled(&agent);
    assert_eq!(resumed.thread_id, before.thread_id);
    assert_eq!(resumed.provider_id, before.provider_id);
    f.shutdown();
    f.assert_no_retire_failure();
}

#[test]
fn daemon_shutdown_does_not_wait_for_a_delayed_native_unload() {
    let mut f = Harness::builder(Provider::Codex)
        .runtime(json!({"sessions_per_codex_process":4,"max_providers":1,"idle_seconds":600}))
        .scenario(json!({"unload_after_unsubscribe_ms":30000}))
        .build();
    f.start();
    let sibling = f.spawn("hold");
    f.wait(&sibling, |a| a.state == "running");
    let agent = f.spawn("idle");
    let before = f.settled(&agent);
    let thread = before.thread_id.as_deref().unwrap();
    f.ok(json!({"op":"stop","target":agent}));
    support::wait::wait_until(
        Duration::from_secs(5),
        Duration::from_millis(20),
        "unsubscribe",
        || {
            f.provider_home
                .join(format!("unsubscribed-{thread}"))
                .exists()
                .then_some(())
        },
    );
    let started = Instant::now();
    f.shutdown();
    assert!(started.elapsed() < Duration::from_secs(20));
    f.assert_no_retire_failure();
    let state = State::new(f.root.clone()).unwrap();
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let binding = state.lookup("main", &profile).unwrap().unwrap();
    let report = serde_json::to_value(state.recovery_report(&binding).unwrap()).unwrap();
    assert!(
        report["executions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["conversation_safe"] != false),
        "{report}"
    );
}

#[test]
fn daemon_shutdown_interrupts_an_unanswered_unload_inventory() {
    let mut f = Harness::builder(Provider::Codex)
        .runtime(json!({"sessions_per_codex_process":4,"max_providers":1,"idle_seconds":600}))
        .scenario(json!({"unload_after_unsubscribe_ms":30000,
            "loaded_list_first_unanswered":true}))
        .build();
    f.start();
    let sibling = f.spawn("hold");
    f.wait(&sibling, |a| a.state == "running");
    let agent = f.spawn("idle");
    f.settled(&agent);
    f.ok(json!({"op":"stop","target":agent}));
    support::wait::wait_until(
        Duration::from_secs(5),
        Duration::from_millis(20),
        "pending list",
        || {
            f.provider_home
                .join("loaded-list-unanswered")
                .exists()
                .then_some(())
        },
    );
    let started = Instant::now();
    f.shutdown();
    assert!(started.elapsed() < Duration::from_secs(10));
    f.assert_no_retire_failure();
    f.start();
    assert!(f.status(&agent).conversation_clean);
    f.shutdown();
}

#[test]
fn pending_native_unload_blocks_purge_until_physical_absence() {
    #[derive(Debug, serde::Deserialize)]
    struct Preview {
        blocked_agents: usize,
        blockers: Vec<Blocker>,
    }
    #[derive(Debug, serde::Deserialize)]
    struct Blocker {
        reason: String,
    }
    let mut f = Harness::builder(Provider::Codex)
        .runtime(json!({"sessions_per_codex_process":4,"max_providers":1,"idle_seconds":600}))
        .scenario(json!({"unload_after_unsubscribe_ms":3000}))
        .build();
    f.start();
    let sibling = f.spawn("idle");
    f.settled(&sibling);
    let agent = f.spawn("idle");
    let before = f.settled(&agent);
    let thread = before.thread_id.as_deref().unwrap();
    f.ok(json!({"op":"terminate","target":agent}));
    support::wait::wait_until(
        Duration::from_secs(5),
        Duration::from_millis(20),
        "unsubscribe",
        || {
            f.provider_home
                .join(format!("unsubscribed-{thread}"))
                .exists()
                .then_some(())
        },
    );
    let preview: Preview =
        serde_json::from_value(f.ok(json!({"op":"purge","target":agent,"dry_run":true}))).unwrap();
    assert_eq!(preview.blocked_agents, 1, "{preview:?}");
    assert_eq!(
        preview.blockers[0].reason, "agent_cleanup_pending",
        "{preview:?}"
    );
    assert!(f.native_file(Provider::Codex, thread).is_file());
    f.wait_purgeable(&agent);
    assert!(f.provider_home.join(format!("unloaded-{thread}")).exists());
    f.ok(json!({"op":"purge","target":agent}));
    f.shutdown();
    f.assert_no_retire_failure();
}

#[test]
fn terminal_cleanup_refusal_still_drains_and_never_claims_safe_release() {
    let mut f = Harness::builder(Provider::Codex)
        .runtime(json!({"sessions_per_codex_process":4,"max_providers":1,"idle_seconds":600}))
        .scenario(json!({"cleanup_refused":true}))
        .build();
    f.start();
    let sibling = f.spawn("hold");
    f.wait(&sibling, |a| a.state == "running");
    let agent = f.spawn("idle");
    f.settled(&agent);
    f.ok(json!({"op":"stop","target":agent}));
    support::wait::wait_until(
        Duration::from_secs(5),
        Duration::from_millis(20),
        "provider drain",
        || (draining_shards(&f) == 1).then_some(()),
    );
    assert_eq!(f.status(&sibling).state, "running");
    assert!(!f.status(&agent).conversation_clean);
    f.shutdown();
    f.assert_recovery_required();
}

#[test]
fn unavailable_or_incomplete_unload_inventory_never_releases_early() {
    for flag in ["loaded_list_error", "loaded_list_cursor"] {
        let mut scenario = json!({"unload_after_unsubscribe_ms":30000});
        scenario[flag] = json!(true);
        let mut f = Harness::builder(Provider::Codex)
            .runtime(json!({"sessions_per_codex_process":4,"max_providers":1,"idle_seconds":600}))
            .scenario(scenario)
            .build();
        f.start();
        let sibling = f.spawn("hold");
        f.wait(&sibling, |a| a.state == "running");
        let agent = f.spawn("idle");
        f.settled(&agent);
        f.ok(json!({"op":"stop","target":agent}));
        support::wait::wait_until(
            Duration::from_secs(5),
            Duration::from_millis(20),
            "drain",
            || (draining_shards(&f) == 1).then_some(()),
        );
        assert!(!f.status(&agent).conversation_clean, "{flag}");
        assert_eq!(f.status(&sibling).state, "running");
        f.shutdown();
    }
}

#[test]
fn crash_during_pending_unload_does_not_publish_clean_conversation() {
    let mut f = Harness::builder(Provider::Codex)
        .runtime(json!({"sessions_per_codex_process":4,"max_providers":1,"idle_seconds":600}))
        .scenario(json!({"unload_after_unsubscribe_ms":30000}))
        .build();
    f.start();
    let sibling = f.spawn("idle");
    f.settled(&sibling);
    let agent = f.spawn("idle");
    let before = f.settled(&agent);
    let thread = before.thread_id.as_deref().unwrap();
    let provider = hive::state::ProcessIdentity::read(before.provider_pid.unwrap())
        .unwrap()
        .unwrap();
    f.ok(json!({"op":"stop","target":agent}));
    support::wait::wait_until(
        Duration::from_secs(5),
        Duration::from_millis(20),
        "unsubscribe",
        || {
            f.provider_home
                .join(format!("unsubscribed-{thread}"))
                .exists()
                .then_some(())
        },
    );
    let mut daemon = f.daemon.take().unwrap();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    support::wait::wait_until(
        Duration::from_secs(10),
        Duration::from_millis(30),
        "provider exit",
        || provider.absent().unwrap().then_some(()),
    );
    f.start();
    let recovered = f.status(&agent);
    assert!(!recovered.conversation_clean, "{recovered:?}");
    f.shutdown();
}

#[test]
fn finished_providers_release_credentials_without_another_ping_or_acquisition() {
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("complete");
    f.wait(&id, |a| {
        a.task_outcome == "completed" && a.state == "stopped"
    });
    let state = State::new(f.root.clone()).unwrap();
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let binding = state.lookup("main", &profile).unwrap().unwrap();
    support::wait::wait_until(
        Duration::from_secs(5),
        Duration::from_millis(10),
        "exited provider releases credentials",
        || match state.operation(&binding, hive::state::OperationKind::Login) {
            Ok(op) => {
                op.finish().unwrap();
                Some(())
            }
            Err(error) => {
                assert_eq!(error.code(), "profile_busy");
                None
            }
        },
    );
    f.shutdown();
}

struct SurvivorRelease {
    stop: PathBuf,
    pid: PathBuf,
    identity: Option<hive::state::ProcessIdentity>,
}
impl Drop for SurvivorRelease {
    fn drop(&mut self) {
        let identity = self.identity.take().or_else(|| {
            fs::read_to_string(&self.pid)
                .ok()
                .and_then(|text| text.parse::<u32>().ok())
                .and_then(|pid| hive::state::ProcessIdentity::read(pid).ok().flatten())
        });
        let _ = fs::write(&self.stop, "stop");
        if let Some(identity) = identity {
            let _ = support::wait::try_wait_until(
                support::wait::STARTUP_TIMEOUT,
                support::wait::POLL_INTERVAL,
                || identity.absent().ok().filter(|absent| *absent),
            );
        }
    }
}
