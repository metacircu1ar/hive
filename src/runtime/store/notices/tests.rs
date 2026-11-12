use super::*;
use crate::runtime::{
    api::{Request, RequestEnvelope},
    caller::Caller,
    preflight::request::Facts,
    test_support::CoreBuilder,
};
use std::os::unix::fs::PermissionsExt;

fn isolated(name: &str) -> bool {
    const CHILD: &str = "HIVE_NOTICE_TEST";
    if std::env::var_os(CHILD).as_deref() == Some(std::ffi::OsStr::new(name)) {
        return false;
    }
    let (_, module) = module_path!().split_once("::").unwrap();
    crate::test_support::run_isolated(
        crate::test_support::rerun_isolated(&format!("{module}::{name}"), false).env(CHILD, name),
    );
    true
}

#[test]
fn scoped_backend_retains_prior_filesystem_cleanup_failures() {
    if isolated("scoped_backend_retains_prior_filesystem_cleanup_failures") {
        return;
    }
    let directory = crate::runtime::test_directory();
    let before = atomic_fs::destructor_cleanup_failures();
    let pending = atomic_fs::PreparedCreate::new(
        directory.path().join("reserved"),
        hive_setup_support::names::PUBLICATION_OPTIONS,
    )
    .unwrap();
    let entries: Vec<_> = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1);
    // A changed preparation must be retained rather than unlinked by Drop.
    std::fs::set_permissions(&entries[0], std::fs::Permissions::from_mode(0o644)).unwrap();
    drop(pending);
    assert!(entries[0].exists());
    assert_eq!(atomic_fs::destructor_cleanup_failures(), before + 1);
    let path = directory.path().join("cleanup.jsonl");
    let (backend, guard) = hive_log::start(&path, hive_log::Config::default()).unwrap();
    backend.scope(|| {
        tracing::info!(name: "test.cleanup_receipt", success = true);
    });
    let finished = guard.finish(std::time::Duration::from_secs(2));
    assert_eq!(finished.state, hive_log::Shutdown::Drained);
    assert_eq!(finished.counts.filesystem_cleanup_failed, before + 1);
    let rows: Vec<serde_json::Value> = std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|row| serde_json::from_str(row).unwrap())
        .collect();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(
        rows.iter()
            .any(|row| row["event"] == "test.cleanup_receipt")
    );
    let loss = rows
        .iter()
        .find(|row| row["event"] == "log.dropped")
        .unwrap();
    assert_eq!(loss["filesystem_cleanup_failed"], before + 1);
}

#[test]
fn auth_notice_keeps_durable_profile_but_logs_only_generation_after_commit() {
    if isolated("auth_notice_keeps_durable_profile_but_logs_only_generation_after_commit") {
        return;
    }
    let directory = crate::runtime::test_directory();
    let path = directory.path().join("auth.jsonl");
    let (backend, guard) = hive_log::start(&path, hive_log::Config::default()).unwrap();
    let store = crate::runtime::test_support::TestStore::new();
    let generation = uuid::Uuid::new_v4();
    let profile = "private-profile-canary";
    let data = serde_json::json!({"profile":profile,"generation":generation});
    backend.scope(|| {
        store.begin().unwrap();
        store
            .auth_event(profile, generation, ProviderEventKind::LoginRestored, &data)
            .unwrap();
        store.rollback().unwrap();
        store.begin().unwrap();
        store
            .auth_event(profile, generation, ProviderEventKind::LoginRestored, &data)
            .unwrap();
        assert!(std::fs::read(&path).unwrap().is_empty());
        store.commit().unwrap();
    });
    assert_eq!(
        guard.finish(std::time::Duration::from_secs(2)).state,
        hive_log::Shutdown::Drained
    );
    let text = std::fs::read_to_string(path).unwrap();
    assert!(!text.contains(profile));
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|r| serde_json::from_str(r).unwrap())
        .collect();
    assert_eq!(rows.len(), 1, "{text}");
    assert_eq!(rows[0]["provider_id"], generation.to_string());
    let saved: (String, String) = store
        .query_row_cached("SELECT provider,data FROM provider_events", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(saved.0, format!("auth:{profile}"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&saved.1).unwrap(),
        data
    );
}

#[test]
fn shutdown_waits_for_outer_commit_and_nested_rollback_restores_the_prior_action() {
    let directory = crate::runtime::test_directory();
    let mut core = CoreBuilder::new(directory.path()).build();
    let request = RequestEnvelope::from_request(Request::Shutdown);
    core.store().begin().unwrap();
    core.dispatch(&Caller::Human, "shutdown", &request, &Facts::default())
        .unwrap();
    assert!(!core.shutdown().load(Ordering::Acquire));
    core.store().rollback().unwrap();
    assert!(!core.shutdown().load(Ordering::Acquire));
    core.store().begin().unwrap();
    core.dispatch(&Caller::Human, "shutdown", &request, &Facts::default())
        .unwrap();
    assert!(!core.shutdown().load(Ordering::Acquire));
    core.store().commit().unwrap();
    assert!(core.shutdown().load(Ordering::Acquire));
}

#[test]
fn a_failed_shutdown_replay_write_does_not_request_shutdown() {
    let directory = crate::runtime::test_directory();
    let mut core = CoreBuilder::new(directory.path()).build();
    core.store().test_connection().execute_batch(
        "CREATE TRIGGER reject_request BEFORE INSERT ON requests BEGIN SELECT RAISE(ABORT, 'fixture'); END;"
    ).unwrap();
    let request = RequestEnvelope::from_request(Request::Shutdown);
    assert!(
        core.dispatch(&Caller::Human, "shutdown", &request, &Facts::default())
            .is_err()
    );
    assert!(!core.shutdown().load(Ordering::Acquire));
}

#[test]
fn shutdown_cannot_be_dropped_by_notice_capacity_or_log_filtering() {
    let flag = Arc::new(AtomicBool::new(false));
    let mut notices = Notices::default();
    notices.begin();
    for _ in 0..=PENDING_LIMIT {
        notices.record(|| Notice::Maintenance {
            gc: false,
            count: 0,
        });
    }
    assert_eq!(notices.dropped, 1);
    notices.request_shutdown(flag.clone());
    notices.begin();
    notices.rollback();
    assert!(!flag.load(Ordering::Acquire));
    notices.commit();
    assert!(flag.load(Ordering::Acquire));
}
