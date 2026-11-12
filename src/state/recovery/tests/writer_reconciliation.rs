//! Writer reconciliation regressions.
use super::*;

#[test]
fn pending_writer_intent_survives_caller_death_and_reconciles_without_authorization() {
    let (_tmp, state, binding) = fixture();
    let status = crate::os::spawn(
        crate::test_support::rerun_isolated("state::recovery::tests::launch_crash_fixture", true)
            .env("HIVE_RECOVERY_TEST_ROOT", &state.root)
            .env("HIVE_RECOVERY_TEST_PHASE", "writer_pending")
            .stdout(std::process::Stdio::null()),
    )
    .and_then(|mut child| child.wait())
    .unwrap();
    assert_eq!(status.code(), Some(79));
    assert!(state.runtime_lease(&binding).is_err());
    let mut operation = state
        .operation(&binding, crate::state::OperationKind::Recover)
        .unwrap();
    complete_reconciliation_probe(&mut operation);
    operation.confirm_missing().unwrap();
    operation.finish().unwrap();
    state.runtime_lease(&binding).unwrap();
}

#[test]
fn an_unrelated_failed_writer_cannot_clear_an_existing_pending_nonce() {
    let (_tmp, state, binding) = fixture();
    let mut op = state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap();
    let first = op
        .prepare_child(true, crate::state::ChildAccess::CredentialWriter)
        .unwrap();
    first.started(std::process::id()).unwrap();
    first.finish(true).unwrap();
    let pending = state.pending_credential_attempt(binding.id).unwrap();
    let second = op
        .prepare_child(true, crate::state::ChildAccess::CredentialWriter)
        .unwrap();
    assert!(second.started(std::process::id()).is_err());
    assert!(second.not_started().is_err());
    assert_eq!(
        state.pending_credential_attempt(binding.id).unwrap(),
        pending
    );
    complete_reconciliation_probe(&mut op);
    op.confirm_missing().unwrap();
}

#[test]
fn unrelated_archived_records_cannot_block_exact_writer_reconciliation() {
    let (_tmp, state, binding) = fixture();
    let mut op = state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap();
    let writer = op
        .prepare_child(true, crate::state::ChildAccess::CredentialWriter)
        .unwrap();
    writer.started(std::process::id()).unwrap();
    writer.finish(true).unwrap();
    let path = binding
        .directory
        .join("execution-archive")
        .join(format!("{}.json", Uuid::new_v4()));
    write_json(
        &path,
        &serde_json::json!({"damaged":"unrelated provider history"}),
    )
    .unwrap();
    complete_reconciliation_probe(&mut op);
    op.confirm_missing().unwrap();
}

#[test]
fn writer_lookup_rejects_symlinked_manifest_and_resolution_directories() {
    for base in ["execution-archive", "credential-resolutions"] {
        let (tmp, state, binding) = fixture();
        let mut op = state
            .operation(&binding, crate::state::OperationKind::Login)
            .unwrap();
        let writer = op
            .prepare_child(true, crate::state::ChildAccess::CredentialWriter)
            .unwrap();
        writer.started(std::process::id()).unwrap();
        writer.finish(true).unwrap();
        drop(writer);
        drop(op);
        let dir = binding.directory.clone();
        let original = dir.join(base);
        if !original.exists() {
            directory(&original).unwrap();
        }
        let mut recover = state
            .operation(&binding, crate::state::OperationKind::Recover)
            .unwrap();
        complete_reconciliation_probe(&mut recover);
        let moved = tmp.path().join("moved");
        fs::rename(&original, &moved).unwrap();
        std::os::unix::fs::symlink(moved, original).unwrap();
        assert!(recover.confirm_missing().is_err());
        assert!(recover.has_pending().unwrap());
        drop(recover);
        assert_eq!(
            state
                .operation(&binding, crate::state::OperationKind::Recover)
                .err()
                .unwrap()
                .code(),
            "unsafe_state"
        );
    }
}

#[test]
fn reconciliation_and_prune_refuse_missing_duplicate_or_replaced_writer_manifest() {
    #[derive(Clone, Copy, Debug)]
    enum Defect {
        Missing,
        Duplicate,
        Nonce,
    }
    for invalid in [Defect::Missing, Defect::Duplicate, Defect::Nonce] {
        let (_tmp, state, binding) = fixture();
        let _fork = crate::os::fork_guard().unwrap();
        let mut operation = state
            .operation(&binding, crate::state::OperationKind::Login)
            .unwrap();
        let child = operation
            .prepare_child(true, crate::state::ChildAccess::CredentialWriter)
            .unwrap();
        child.started(std::process::id()).unwrap();
        child.finish(true).unwrap();
        drop(child);
        drop(operation);
        let attempt = state
            .pending_credential_attempt(binding.id)
            .unwrap()
            .unwrap();
        let dir = binding.directory.clone();
        let path = dir.join(format!("execution-archive/{}.json", attempt.writer));
        let record: ledger::Record = read_json(&path).unwrap();
        match invalid {
            Defect::Missing => fs::remove_file(path).unwrap(),
            Defect::Duplicate => {
                directory(&dir.join("writers")).unwrap();
                write_json(
                    &dir.join(format!("writers/{}.json", attempt.writer)),
                    &record,
                )
                .unwrap();
            }
            Defect::Nonce => {
                let mut changed = record;
                changed.run = Uuid::new_v4();
                assert_ne!(changed.run, attempt.run);
                write_json(&path, &changed).unwrap();
            }
        }
        assert!(
            state
                .operation(&binding, crate::state::OperationKind::Recover)
                .is_err()
        );
        assert!(
            state
                .operation(&binding, crate::state::OperationKind::Prune)
                .is_err()
        );
    }
}
