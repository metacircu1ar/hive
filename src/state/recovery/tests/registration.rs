//! Registration regressions.
use super::*;

#[test]
fn missing_ready_credential_lock_preserves_recovery_without_recreating_evidence() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let path = binding.directory.join("auth.lock");
    fs::remove_file(&path).unwrap();
    for kind in [
        OperationKind::Status,
        OperationKind::Models,
        OperationKind::SteeringCanary,
        OperationKind::Login,
        OperationKind::Recover,
    ] {
        assert_eq!(
            state.operation(&binding, kind).err().unwrap().code(),
            "recovery_required"
        );
        assert!(!path.exists());
    }
    assert_eq!(
        state.runtime_lease(&binding).err().unwrap().code(),
        "recovery_required"
    );
    let report = serde_json::to_value(state.recovery_report(&binding).unwrap()).unwrap();
    assert_eq!(report["diagnostic"]["code"], "recovery_required");
    assert_eq!(report["runtime_usable"], false);
    assert_eq!(report["read_only"], true);
    assert_eq!(
        ledger::reconcile(&state, &binding).unwrap_err().code(),
        "recovery_required"
    );
    assert_eq!(
        state
            .operation(&binding, OperationKind::Prune)
            .err()
            .unwrap()
            .code(),
        "state_io"
    );
    assert!(!path.exists());
}

#[test]
fn repair_cutover_cannot_recreate_a_missing_candidate_lock() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let repair = state.begin_repair("main", &binding.profile).unwrap();
    let path = repair.binding.directory.join("auth.lock");
    fs::remove_file(&path).unwrap();
    assert_eq!(
        state.finish_repair(&repair, true, None).unwrap_err().code(),
        "recovery_required"
    );
    assert!(!path.exists());
    assert_eq!(
        state
            .selected_generation("main", &binding.profile)
            .unwrap()
            .id,
        binding.id
    );
}

#[test]
fn never_ready_registration_rejects_repair_and_reconcile_without_creating_locks() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let pending = crate::state::test_support::edit_index(&state, |index| {
        index.profiles.get_mut("main").unwrap().ready = false;
        let generation = index.generations.get_mut(&binding.id).unwrap();
        generation.binding.ready = false;
        generation.credential_lock = None;
        generation.binding.clone()
    });
    fs::remove_file(binding.directory.join("auth.lock")).unwrap();
    assert_eq!(
        state
            .begin_repair("main", &binding.profile)
            .err()
            .unwrap()
            .code(),
        "registration_incomplete"
    );
    assert_eq!(
        ledger::reconcile(&state, &pending).unwrap_err().code(),
        "registration_incomplete"
    );
    assert_eq!(
        state.recovery_report(&pending).unwrap_err().code(),
        "registration_incomplete"
    );
    assert!(!binding.directory.join("auth.lock").exists());
}

#[test]
fn claude_registration_without_user_environment_finishes() {
    use std::{io::Read, process::Stdio, time::Duration};

    for user in [None, Some("")] {
        for test in [
            "claude_namespace_is_normalized_and_keychain_account_stays_pinned",
            "claude_index_rejects_missing_pinned_keychain_account",
        ] {
            let name = format!("state::recovery::tests::{test}");
            let mut command = crate::test_support::rerun_isolated(&name, false);
            if let Some(user) = user {
                command.env("USER", user);
            } else {
                command.env_remove("USER");
            }
            let mut child = crate::test_support::ChildGuard(
                crate::os::spawn(command.stdout(Stdio::piped())).unwrap(),
            );
            // Wait before reading: a recursive fork lock must fail within a
            // deadline, with ChildGuard still owning kill/reap on timeout.
            let status = crate::test_support::wait_until(
                Duration::from_secs(20),
                Duration::from_millis(10),
                || child.try_wait().unwrap(),
            );
            let mut output = String::new();
            child
                .stdout
                .take()
                .unwrap()
                .take(64 * 1024)
                .read_to_string(&mut output)
                .unwrap();
            assert!(
                status.success()
                    && output.lines().any(|line| line.starts_with(
                        "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; "
                    )),
                "{name} with USER={user:?}: {status}; {output}"
            );
        }
    }
}
