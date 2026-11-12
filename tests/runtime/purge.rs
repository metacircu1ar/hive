use super::*;
use std::path::Path;

#[test]
fn purge_claude_session_in_long_nfd_and_astral_working_directory() {
    let mut f = claude::fixture();
    let cwd = f
        .tmp
        .path()
        .join(format!("cafe\u{301}-{}-\u{1f600}", "x".repeat(180)));
    hive::state::directory(&cwd).unwrap();
    f.start();
    let spawned = f.spawned(json!({"op":"spawn", "role":"claude", "cwd":cwd, "body":"hold"}));
    let id = spawned.agent_id.as_str();
    f.wait(id, |a| a.state == "running");
    f.ok(json!({"op":"terminate", "target":id}));
    f.wait_purgeable(id);
    let preview = f.ok(json!({"op":"purge", "target":id, "dry_run":true}));
    assert_eq!(preview["native_warning_count"], 0, "{preview}");
    let path = preview["native_preview"][0]["files"][0]["path"]
        .as_str()
        .unwrap();
    let native = f.provider_home(Provider::Claude).join(path);
    assert!(native.is_file());
    let removed = f.ok(json!({"op":"purge", "target":id}));
    assert_eq!(removed["native_sessions_deleted"], 1, "{removed}");
    assert!(!native.exists());
    f.shutdown();
}

#[test]
fn missing_optional_codex_path_allows_startup_and_warns_at_purge() {
    let mut f = Harness::new();
    f.set_scenario(Provider::Codex, json!({"omit_native_path":true}));
    f.start();
    let id = f.spawn("hold");
    let status = f.wait(&id, |a| a.state == "running");
    let native = f.native_file(Provider::Codex, status.thread_id.as_deref().unwrap());
    assert!(native.exists());
    f.ok(json!({"op":"terminate", "target":id}));
    f.wait_purgeable(&id);
    let result = f.ok(json!({"op":"purge", "target":id}));
    assert_eq!(result["native_history"], "partial", "{result}");
    assert_eq!(result["native_sessions_retained"], 1);
    assert_eq!(result["native_sessions_absent"], 0);
    assert_eq!(result["native_warnings"][0]["reason"], "incomplete_history");
    assert!(native.exists());
    f.shutdown();
}

pub(super) fn purgeable(f: &Harness, id: &str) {
    f.wait_purgeable(id);
}

provider_matrix!(
    purge_cli_removes_owned_native_history_and_keeps_shared_state,
    purge_cli_removes_owned_native_history_and_keeps_shared_state_case
);

fn purge_cli_removes_owned_native_history_and_keeps_shared_state_case(provider: Provider) {
    let claude_model = provider == Provider::Claude;
    let mut f = if claude_model {
        claude::fixture()
    } else {
        Harness::new()
    };
    f.start();
    let id = if claude_model {
        claude::spawn(&f, "hold")
    } else {
        f.spawn("hold")
    };
    f.wait(&id, |a| a.state == "running");
    f.ok(json!({"op":"terminate","target":id}));
    f.wait(&id, |a| a.state == "stopped");
    purgeable(&f, &id);
    let provider = if claude_model {
        Provider::Claude
    } else {
        Provider::Codex
    };
    let sentinel = f.provider_home(provider).join("retained-native-history");
    fs::write(&sentinel, "retained").unwrap();
    let thread = f.status(&id).thread_id.as_deref().unwrap().to_owned();
    let native = f.native_file(provider, &thread);
    assert!(native.is_file());
    let result = purge_and_check_native(&f, &id, &native, &sentinel);
    f.shutdown();
    f.start();
    let retry = f.call_id(
        "cli-purge",
        json!({"op":"purge","target":id,"recursive":false,"dry_run":false,
            "keep_native_history":false}),
    );
    assert_eq!(retry, result);
    assert_eq!(
        support::reply::Failure::parse(f.call(json!({"op":"purge","target":id})))
            .error
            .code,
        "agent_purged"
    );
    f.shutdown();
}

#[test]
fn purge_released_shared_member_preserves_peer_and_later_provider_shutdown() {
    let mut f = Harness::new();
    configure_runtime(
        &f,
        json!({"max_loaded":2,"max_providers":1,"sessions_per_codex_process":2,"max_starting":2,
            "max_active_turns":2,"idle_seconds":600}),
        false,
    );
    f.start();
    let removed = f.spawn("hold");
    let peer = f.spawn("hold");
    let a = f.wait(&removed, |a| a.state == "running");
    let b = f.wait(&peer, |a| a.state == "running");
    assert_eq!(a.provider_id, b.provider_id);
    f.ok(json!({"op":"terminate","target":removed}));
    f.wait(&removed, |a| a.state == "stopped");
    purgeable(&f, &removed);
    let purged = f.call(json!({"op":"purge","target":removed}));
    assert_eq!(purged["counts"]["agents"], 1, "{purged}");
    assert_eq!(
        f.call(json!({"op":"status","target":peer}))["agent"]["state"],
        "running"
    );
    f.ok(json!({"op":"terminate","target":peer}));
    f.wait(&peer, |a| a.state == "stopped");
    let events = f.read(&peer);
    assert!(
        events
            .events
            .iter()
            .any(|e| e.kind == "provider_shutdown" && e.data["confirmed"] == true),
        "{events:?}"
    );
    f.shutdown();
    let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
    let provider = b.provider_id.as_deref().unwrap();
    let shutdown: String = db
        .query_row(
            "SELECT data FROM provider_events WHERE provider=?1 AND kind='shutdown'",
            [provider],
            |r| r.get(0),
        )
        .unwrap();
    let shutdown: Value = serde_json::from_str(&shutdown).unwrap();
    assert_eq!(shutdown["shutdown"]["confirmed"], true);
}

#[test]
fn purge_cleanup_failure_returns_retry_id_and_recovers_with_or_without_restart() {
    use std::os::unix::fs::symlink;
    for restart in [false, true] {
        let mut f = Harness::new();
        f.start();
        let id = f.spawn("complete");
        f.wait(&id, |a| {
            a.state == "stopped" && a.task_outcome == "completed"
        });
        purgeable(&f, &id);
        let thread = f.status(&id).thread_id.as_deref().unwrap().to_owned();
        let native = f.native_file(Provider::Codex, &thread);
        f.shutdown();
        let dir = f.root.join("runtime/output");
        let first = "00000000-0000-4000-8000-000000000001";
        let second = "00000000-0000-4000-8000-000000000002";
        let file = dir.join(format!("{first}.json"));
        fs::write(&file, "one\n").unwrap();
        fs::set_permissions(file, fs::Permissions::from_mode(0o600)).unwrap();
        let outside = f.tmp.path().join("retained");
        fs::write(&outside, "untouched").unwrap();
        let unsafe_file = dir.join(format!("{second}.json"));
        symlink(&outside, &unsafe_file).unwrap();
        let manifest = f.root.join("runtime/output-manifest.jsonl");
        fs::write(&manifest, "derived manifest\n").unwrap();
        fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
        {
            let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
            for artifact in [first, second] {
                db.execute(
                    "INSERT INTO artifacts(id,bytes,created_at,agent) VALUES(?1,4,0,?2)",
                    [artifact, &id],
                )
                .unwrap();
            }
        }
        f.start();
        let args = json!({"op":"purge","target":id});
        let failed = f.call_id("retry-cleanup", args.clone());
        let failed = support::reply::Failure::parse(failed);
        assert_eq!(failed.error.code, "purge_cleanup_pending", "{failed:?}");
        assert_eq!(
            failed.error.message,
            "Agent records were purged but output or native-session cleanup is incomplete. \
                Retry with the returned \
                request_id."
        );
        assert_eq!(failed.request_id.as_ref().unwrap(), "retry-cleanup");
        assert_eq!(
            support::reply::Failure::parse(f.call(json!({"op":"status","target":id})))
                .error
                .code,
            "agent_purged"
        );
        assert!(!dir.join(format!("{first}.json")).exists());
        assert!(native.exists()); // Artifact failure precedes native cleanup.
        assert_eq!(fs::read_to_string(&outside).unwrap(), "untouched");
        fs::remove_file(&unsafe_file).unwrap();
        if restart {
            f.shutdown();
            f.start();
        }
        let retried = f.call_id("retry-cleanup", args);
        assert_eq!(retried["counts"]["artifacts"], 2, "{retried}");
        assert_eq!(retried["native_sessions_deleted"], 1, "{retried}");
        assert!(!native.exists());
        assert!(!manifest.exists());
        f.shutdown();
        let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM artifact_deletions", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

fn purge_with_cli(f: &Harness, id: &str, dry: bool) -> Value {
    let mut cmd = support::command(env!("CARGO_BIN_EXE_hivectl"));
    cmd.arg("--state-dir")
        .arg(&f.root)
        .args(["purge", id, "--request-id", "cli-purge"]);
    if dry {
        cmd.arg("--dry-run");
    }
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice::<Value>(&output.stdout).unwrap()
}

fn purge_and_check_native(f: &Harness, id: &str, native: &Path, sentinel: &Path) -> Value {
    let preview = purge_with_cli(f, id, true);
    assert_eq!(preview["selected_agents"], 1);
    assert_eq!(preview["blocked_agents"], 0);
    let result = purge_with_cli(f, id, false);
    assert_eq!(result["counts"]["agents"], 1);
    assert_eq!(result["native_history"], "deleted");
    assert_eq!(result["native_sessions_deleted"], 1);
    assert!(!native.exists());
    assert_eq!(result["request_id"], "cli-purge");
    assert!(sentinel.exists());
    assert!(f.tmp.path().join("role.md").exists());
    for op in ["status", "read", "messages", "terminate"] {
        f.expect_err(json!({"op":op,"target":id}), "agent_purged");
    }
    result
}
