//! Layout validation regressions.
use super::*;

#[test]
fn unknown_namespace_layout_blocks_ownership_decisions_without_writes() {
    for name in ["writers-v2", "unrecognized-evidence"] {
        let (_tmp, state, binding) = fixture();
        let _fork = crate::os::fork_guard().unwrap();
        state.runtime_lease(&binding).unwrap();
        let mut operation = state
            .operation(&binding, crate::state::OperationKind::Login)
            .unwrap();
        let child = operation
            .prepare_child(true, crate::state::ChildAccess::CredentialWriter)
            .unwrap();
        child.started(std::process::id()).unwrap();
        drop(child);
        drop(operation);
        let dir = binding.directory.clone();
        fs::rename(dir.join("writers"), dir.join(name)).unwrap();
        let before = snapshot_files(&state.root);
        let assert_layout = |error: Error| assert_eq!(error.code(), "unsupported_schema");
        assert_layout(state.runtime_lease(&binding).err().unwrap());
        for kind in [
            crate::state::OperationKind::Status,
            crate::state::OperationKind::Login,
            crate::state::OperationKind::Prune,
        ] {
            assert_layout(state.operation(&binding, kind).err().unwrap());
        }
        assert_layout(state.begin_repair("main", &binding.profile).err().unwrap());
        assert_layout(state.register("main", &binding.profile).err().unwrap());
        assert_layout(state.recovery_report(&binding).unwrap_err());
        assert_layout(
            state
                .check_conversation(&binding, Some(&Uuid::new_v4().to_string()))
                .unwrap_err(),
        );
        assert_layout(state.prune("main", binding.id).unwrap_err());
        let reservations = state.recovery_reservations().unwrap();
        assert_eq!(reservations.len(), 1);
        assert!(reservations[0].unknown_capacity && reservations[0].unresolved_activity);
        assert_eq!(snapshot_files(&state.root), before);
    }
}

#[test]
fn namespace_layout_rejects_wrong_types_and_accepts_current_staging_files() {
    let (_tmp, state, binding) = fixture();
    let dir = binding.directory.clone();
    let temporary = atomic_fs::PreparedCreate::new(
        dir.join("fixture.json"),
        hive_setup_support::names::PUBLICATION_OPTIONS,
    )
    .unwrap();
    layout::profile(&dir).unwrap();
    temporary.cancel().unwrap();
    for name in ["writers", "executions", "execution-archive"] {
        crate::state::write_json(&dir.join(name), &serde_json::json!({})).unwrap();
        assert_eq!(layout::profile(&dir).unwrap_err().code(), "unsafe_state");
        fs::remove_file(dir.join(name)).unwrap();
        directory(&dir.join(name)).unwrap();
    }
    state.runtime_lease(&binding).unwrap();
}

#[test]
fn interrupted_atomic_write_staging_files_are_not_ownership_records() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    for name in ["", "writers", "executions"] {
        let dir = binding.directory.join(name);
        directory(&dir).unwrap();
        for name in [".hive-tmp-ABC123", ".hive-tmp-0123456789abcdef"] {
            crate::state::private_file(&dir.join(name), true)
                .unwrap()
                .write_all(b"partial")
                .unwrap();
        }
    }
    state.runtime_lease(&binding).unwrap();
    state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap()
        .finish()
        .unwrap();
    assert!(crate::api::value(state.recovery_report(&binding).unwrap())["history_error"].is_null());
    let unknown = binding.directory.join("writers/.unknown");
    write_json(&unknown, &serde_json::json!({})).unwrap();
    assert!(state.runtime_lease(&binding).is_err());
}

#[test]
fn obsolete_or_malformed_staging_names_cannot_bypass_ownership_validation() {
    for name in [
        ".tmpABC123",
        ".hive-tmp-ABCDE",
        ".hive-tmp-0123456789abcdeF",
    ] {
        for relative in ["", "writers"] {
            let (_tmp, state, binding) = fixture();
            let _fork = crate::os::fork_guard().unwrap();
            let dir = binding.directory.join(relative);
            directory(&dir).unwrap();
            let path = dir.join(name);
            crate::state::private_file(&path, true)
                .unwrap()
                .write_all(b"partial")
                .unwrap();
            assert!(state.runtime_lease(&binding).is_err(), "{relative}/{name}");
            assert_eq!(fs::read(path).unwrap(), b"partial");
        }
    }
}
