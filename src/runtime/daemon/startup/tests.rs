use super::*;
use crate::runtime::Binaries;

#[test]
fn invalid_logging_limits_are_rejected_before_state_creation() {
    let temp = crate::runtime::test_directory();
    let config = temp.path().join("invalid.yaml");
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .create_new(&config, b"auth_profiles: {}\nlogging: {files: 1}\n")
        .unwrap();
    let root = temp.path().join("state");
    let error = OwnedDaemon::open(config, root.clone(), Binaries::default())
        .err()
        .unwrap();
    assert_eq!(error.code(), "config_invalid");
    assert!(!root.exists());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    let missing_parent = temp.path().join("missing");
    assert!(
        OwnedDaemon::open(
            temp.path().join("invalid.yaml"),
            missing_parent.join("state"),
            Binaries::default()
        )
        .is_err()
    );
    assert!(!missing_parent.exists());
}

#[test]
fn startup_sweep_requires_idle_owner_and_original_lock_evidence() {
    for kind in ["held", "live", "replaced", "idle"] {
        let mut fork = Some(crate::os::fork_guard().unwrap());
        let temp = crate::runtime::test_directory();
        let root = temp.path().join("state");
        let runtime = root.join("runtime");
        drop(State::new(root.clone()).unwrap().initialize().unwrap());
        state::directory(&runtime).unwrap();
        let config = temp.path().join("config.yaml");
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .create_new(&config, b"auth_profiles: {}\n")
            .unwrap();
        let lock_path = runtime.join("daemon.lock");
        let lock = Lock::acquire(&lock_path).unwrap();
        let candidate = runtime.join(".hive-tmp-state-ABC123");
        state::private_file(&candidate, true).unwrap();
        if kind == "live" {
            write_daemon_owner(
                &runtime.join("owner.json"),
                &state::boot_identity().unwrap(),
                &lock,
                &"0".repeat(64),
            )
            .unwrap();
        }
        let held = (kind == "held").then_some(lock);
        if kind == "replaced" {
            let mut command = crate::test_support::rerun_isolated(
                "state::daemon::owner_tests::write_owner_marker_fixture",
                true,
            );
            command.env("HIVE_OWNER_MARKER_TEST_ROOT", &runtime);
            drop(fork.take());
            crate::test_support::run_isolated(&mut command);
            fork = Some(crate::os::fork_guard().unwrap());
            fs::rename(&lock_path, runtime.join("old.lock")).unwrap();
            state::private_file(&lock_path, true).unwrap();
        }
        let opened = OwnedDaemon::open(
            config,
            root,
            Binaries {
                codex: "/missing-fixture-codex".into(),
                claude: "/missing-fixture-claude".into(),
            },
        );
        if kind == "idle" {
            opened.unwrap();
            assert!(!candidate.exists());
        } else {
            let code = opened.err().unwrap().code();
            assert_eq!(
                code,
                if kind == "held" {
                    "daemon_busy"
                } else {
                    "recovery_required"
                }
            );
            assert!(candidate.exists());
        }
        drop(held);
        drop(fork);
    }
}

#[cfg(feature = "comb")]
#[test]
fn recovery_uses_current_collector_configuration_atomically() {
    use crate::{
        config::CollectionSource,
        runtime::{
            domain::{AgentState, EventKind},
            test_support::AgentBuilder,
        },
    };
    for (was_enabled, enabled, selected, expected) in [
        (false, true, true, 1),
        (true, false, true, 0),
        (true, true, false, 0),
    ] {
        let temp = crate::runtime::test_directory();
        let mut config = Config::parse("auth_profiles: {}\n").unwrap();
        let settings = crate::config::CollectorConfig {
            credential_file: temp.path().join("absent-credential.json"),
            enabled: was_enabled,
            sources: [CollectionSource::TurnFailed].into(),
        };
        config.comb_collectors.insert("shared".into(), settings);
        let store = super::super::Store::open(&temp.path().join("runtime.sqlite")).unwrap();
        store
            .configure_collectors(&config.comb_collectors, &std::collections::BTreeMap::new())
            .unwrap();
        let a = AgentBuilder::new("agent")
            .configure(|a| {
                a.state = AgentState::Running;
                a.execution_id = Some("execution".into());
                a.turn_id = Some("turn".into());
            })
            .build();
        store.save(&a).unwrap();
        store
            .event(&a, EventKind::ExecutionStarting, serde_json::json!({}))
            .unwrap();
        drop(store);
        let settings = config.comb_collectors.get_mut("shared").unwrap();
        settings.enabled = enabled;
        if !selected {
            settings.sources = [CollectionSource::Review].into();
        }
        let recovered = recover_store(temp.path(), &config).unwrap();
        assert_eq!(recovered.collector_status().unwrap()[0].pending, expected);
        recovered.recover_agents().unwrap();
        assert_eq!(recovered.collector_status().unwrap()[0].pending, expected);
        let bodies: Vec<String> = recovered
            .test_connection()
            .prepare("SELECT body FROM comb_outbox")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        if expected != 0 {
            let body: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
            assert_eq!(
                body["record"]["evidence"],
                serde_json::json!({"outcome":"uncertain"})
            );
            assert_eq!(body["record"]["turn"], "turn");
        }
    }
}
