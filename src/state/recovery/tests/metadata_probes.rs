//! Metadata probes regressions.
use super::*;

#[test]
fn shared_probes_do_not_create_writers_and_weak_lease_release_allows_login() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let lease = Arc::new(state.runtime_lease(&binding).unwrap());
    let weak = Arc::downgrade(&lease);
    let mut probe = state
        .operation(&binding, crate::state::OperationKind::Status)
        .unwrap();
    let child = probe
        .prepare_child(true, crate::state::ChildAccess::Probe)
        .unwrap();
    child.started(std::process::id()).unwrap();
    child.finish(true).unwrap();
    drop(child);
    probe.finish().unwrap();
    assert!(matches!(
        state.operation(&binding, crate::state::OperationKind::Login),
        Err(Error::ProfileBusy { .. })
    ));
    drop(lease);
    assert!(weak.upgrade().is_none());
    state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap()
        .finish()
        .unwrap();
}

#[test]
fn live_verification_probe_cannot_clear_pending_credentials() {
    use std::os::unix::process::CommandExt;
    let (_tmp, state, binding) = fixture();
    let mut op = state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap();
    let writer = op
        .prepare_child(true, crate::state::ChildAccess::CredentialWriter)
        .unwrap();
    writer.started(std::process::id()).unwrap();
    writer.finish(true).unwrap();
    let probe = op
        .prepare_child(true, crate::state::ChildAccess::Probe)
        .unwrap();
    let mut child = crate::test_support::ChildGuard(
        crate::os::spawn(
            std::process::Command::new("/bin/sleep")
                .arg("10")
                .process_group(0),
        )
        .unwrap(),
    );
    probe.started(child.id()).unwrap();
    assert!(op.confirm_missing().is_err());
    child.kill().unwrap();
    child.wait().unwrap();
    probe.finish(false).unwrap();
    op.confirm_missing().unwrap();
    assert!(
        state
            .pending_credential_attempt(binding.id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn standalone_metadata_probes_never_grant_credential_writer_authority() {
    let (_tmp, state, binding) = fixture();
    let _busy = state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap();
    let mut metadata = state.metadata_probe().unwrap();
    assert!(
        metadata
            .prepare_child(true, crate::state::ChildAccess::CredentialWriter)
            .is_err()
    );
    assert!(metadata.confirm_missing().is_err());
    assert!(metadata.confirm_account(None).is_err());
    complete_reconciliation_probe(&mut metadata);
    metadata.verify_probe_ownership().unwrap();
    metadata.finish().unwrap();
}

#[test]
fn metadata_probes_reject_symlinked_state_without_touching_destination() {
    let tmp = tempfile::tempdir().unwrap();
    let destination = tmp.path().join("destination");
    directory(&destination).unwrap();
    let root = tmp.path().join("state");
    std::os::unix::fs::symlink(&destination, &root).unwrap();
    let state = State::new(root).unwrap();
    assert!(state.metadata_probe().is_err());
    assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
}
