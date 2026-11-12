//! Generation repair regressions.
use super::*;

#[test]
fn replaced_credential_lock_is_not_a_runtime_or_mutation_bypass() {
    let (tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let lease = state.runtime_lease(&binding).unwrap();
    let path = binding.directory.join("auth.lock");
    fs::rename(&path, tmp.path().join("retained-auth.lock")).unwrap();
    assert!(lease.verify(&state, &binding).is_err());
    assert!(state.runtime_lease(&binding).is_err());
    assert!(
        state
            .operation(&binding, crate::state::OperationKind::Login)
            .is_err()
    );
    drop(lease);
    let repair = state.begin_repair("main", &binding.profile).unwrap();
    state.runtime_lease(&repair.binding).unwrap();
}

#[test]
fn corrupt_runtime_history_is_reported_and_does_not_invalidate_credentials() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    state.runtime_lease(&binding).unwrap();
    let base = binding.directory.join("executions");
    directory(&base).unwrap();
    let path = base.join(format!("{}.json", Uuid::new_v4()));
    write_json(&path, &serde_json::json!({"unreadable":"record"})).unwrap();
    state.runtime_lease(&binding).unwrap();
    let report = crate::api::value(state.recovery_report(&binding).unwrap());
    assert_eq!(report["runtime_usable"], true);
    assert!(report["history_error"].is_object());
    assert!(
        state
            .operation(&binding, crate::state::OperationKind::Login)
            .is_err()
    );
    assert!(state.recovery_reservations().unwrap()[0].unknown_capacity);
}

#[test]
fn concurrent_repair_cas_and_cancel_preserve_every_namespace() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let a = state.begin_repair("main", &binding.profile).unwrap();
    let b = state.begin_repair("main", &binding.profile).unwrap();
    assert!(state.finish_repair(&a, true, None).unwrap());
    assert!(!state.finish_repair(&b, true, None).unwrap());
    let c = state.begin_repair("main", &binding.profile).unwrap();
    assert!(!state.finish_repair(&c, false, None).unwrap());
    assert_eq!(
        state.lookup("main", &binding.profile).unwrap().unwrap().id,
        a.binding.id
    );
    let generations = state.generations("main").unwrap();
    assert_eq!(generations.len(), 4);
    assert_eq!(
        generations
            .iter()
            .filter(|g| g.state == GenerationState::Retired)
            .count(),
        3
    );
    assert!(state.finish_repair(&b, true, None).is_err());
    state.prune("main", b.binding.id).unwrap();
    assert!(b.binding.directory.join("prune-receipt.json").exists());
    assert!(state.prune("main", a.binding.id).is_err());
}

#[test]
fn unfinished_writer_requires_new_generation_without_changing_old_binding() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let mut operation = state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap();
    operation
        .prepare_child(true, crate::state::ChildAccess::CredentialWriter)
        .unwrap()
        .started(std::process::id())
        .unwrap();
    drop(operation);
    assert_eq!(
        state.runtime_lease(&binding).err().unwrap().code(),
        "recovery_required"
    );
    let repair = state.begin_repair("main", &binding.profile).unwrap();
    assert_ne!(repair.binding.directory, binding.directory);
    state.runtime_lease(&repair.binding).unwrap();
    assert!(
        state
            .finish_repair(&repair, true, Some("account-a".into()))
            .unwrap()
    );
    assert_eq!(
        state.lookup("main", &binding.profile).unwrap().unwrap().id,
        repair.binding.id
    );
    assert_eq!(
        state
            .generation("main", &binding.profile, binding.id)
            .unwrap()
            .id,
        binding.id
    );
    assert!(state.runtime_lease(&binding).is_err());
    let fresh = state.runtime_lease(&repair.binding).unwrap();
    assert!(fresh.verify_account(Some("account-b")).is_err());
    fresh.verify_account(None).unwrap(); // Lack of native identity is not invented proof.
    assert!(state.runtime_lease(&binding).is_err());
}

#[test]
fn corrupt_writer_history_blocks_only_old_generation() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let dir = binding.directory.join("writers");
    directory(&dir).unwrap();
    write_json(&dir.join("bad.json"), &serde_json::json!({})).unwrap();
    assert!(state.runtime_lease(&binding).is_err());
    let repair = state.begin_repair("main", &binding.profile).unwrap();
    state.runtime_lease(&repair.binding).unwrap();
}

#[test]
fn cutover_rejects_a_replaced_candidate_lock_before_retiring_active_generation() {
    let (tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let repair = state.begin_repair("main", &binding.profile).unwrap();
    state.runtime_lease(&repair.binding).unwrap();
    let lock = repair.binding.directory.join("auth.lock");
    fs::rename(&lock, tmp.path().join("retained-auth.lock")).unwrap();
    assert!(state.finish_repair(&repair, true, None).is_err());
    assert_eq!(
        state.lookup("main", &binding.profile).unwrap().unwrap().id,
        binding.id
    );
}

#[test]
fn completed_writer_stays_fenced_until_account_verification_and_failed_repair_keeps_evidence() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    state
        .record_account(&binding, Some("before".into()))
        .unwrap();
    let mut op = state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap();
    let receipt = op
        .prepare_child(true, crate::state::ChildAccess::CredentialWriter)
        .unwrap();
    receipt.started(std::process::id()).unwrap();
    receipt.finish(true).unwrap();
    drop(receipt);
    op.finish().unwrap();
    assert!(state.runtime_lease(&binding).is_err());
    let g = state.generations("main").unwrap().remove(0);
    assert!(g.possibly_authenticated && g.credential_mutation.is_some());
    assert!(
        state
            .finish_credential_attempt(
                crate::state::CredentialAttempt {
                    generation: binding.id,
                    mutation: g.credential_mutation.unwrap(),
                    writer: g.credential_writer.unwrap()
                },
                crate::state::CredentialCompletion::Authenticated(Some("after".into()))
            )
            .is_err()
    );
    let repair = state.begin_repair("main", &binding.profile).unwrap();
    state
        .record_account(&repair.binding, Some("new".into()))
        .unwrap();
    state.finish_repair(&repair, false, None).unwrap();
    let retired = state
        .generations("main")
        .unwrap()
        .into_iter()
        .find(|g| g.binding.id == repair.binding.id)
        .unwrap();
    assert!(retired.authenticated && retired.possibly_authenticated);
    assert_eq!(retired.account_identity.as_deref(), Some("new"));
}
