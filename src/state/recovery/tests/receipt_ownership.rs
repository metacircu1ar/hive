//! Receipt ownership regressions.
use super::*;

#[test]
fn runtime_failure_fences_only_its_conversation_and_preserves_evidence() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let lease = Arc::new(state.runtime_lease(&binding).unwrap());
    let id = Uuid::new_v4().to_string();
    let mut evidence = lease.begin_provider(&id).unwrap();
    evidence.started(std::process::id()).unwrap();
    evidence
        .finish(crate::state::ProcessCompletion {
            quiescent: false,
            confirmed: false,
        })
        .unwrap();
    assert!(state.check_conversation(&binding, Some(&id)).is_err());
    drop(evidence);
    drop(lease);
    state.runtime_lease(&binding).unwrap();
    state
        .operation(&binding, crate::state::OperationKind::Status)
        .unwrap()
        .finish()
        .unwrap();
    assert_eq!(
        state
            .operation(&binding, crate::state::OperationKind::Login)
            .err()
            .unwrap()
            .code(),
        "recovery_required"
    );
    let report = crate::api::value(state.recovery_report(&binding).unwrap());
    assert_eq!(report["runtime_usable"], true);
    assert_eq!(report["executions"][0]["quiescent"], false);
    assert!(
        binding
            .directory
            .join(format!("executions/{id}.json"))
            .exists()
    );
}

#[test]
fn missing_execution_receipt_cannot_establish_clean_release() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let id = Uuid::new_v4().to_string();
    assert!(state.check_conversation(&binding, Some(&id)).is_err());
    let lease = Arc::new(state.runtime_lease(&binding).unwrap());
    let mut evidence = lease.begin_provider(&id).unwrap();
    evidence
        .finish(crate::state::ProcessCompletion {
            quiescent: true,
            confirmed: true,
        })
        .unwrap();
    state.check_conversation(&binding, Some(&id)).unwrap();
    let archive = binding
        .directory
        .join(format!("execution-archive/{id}.json"));
    assert!(archive.exists());
}

#[test]
fn stale_receipt_cannot_overwrite_another_execution_revision() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let lease = Arc::new(state.runtime_lease(&binding).unwrap());
    let id = Uuid::new_v4().to_string();
    let mut receipt = lease.begin_provider(&id).unwrap();
    let path = binding.directory.join(format!("executions/{id}.json"));
    let mut value: serde_json::Value = read_json(&path).unwrap();
    value["run"] = Uuid::new_v4().to_string().into();
    write_json(&path, &value).unwrap();
    assert!(
        receipt
            .finish(crate::state::ProcessCompletion {
                quiescent: true,
                confirmed: true
            })
            .is_err()
    );
    let saved: serde_json::Value = read_json(&path).unwrap();
    assert_eq!(saved, value);
}

#[test]
fn pid_birth_identity_rejects_malformed_identity_and_does_not_signal_reused_pid() {
    let current = ProcessIdentity::current().unwrap();
    assert!(!current.absent().unwrap());
    let mut prior = current;
    prior.birth = "malformed".into();
    assert!(prior.absent().is_err());
    #[cfg(target_os = "macos")]
    {
        prior.birth = "macos:1:0".into();
    }
    #[cfg(target_os = "linux")]
    {
        prior.birth = "linux:1".into();
    }
    assert!(prior.absent().unwrap());
    assert!(ProcessIdentity::read(0).is_err());
    assert!(crate::state::same_boot("invalid").is_err());
    assert!(crate::state::same_boot(&crate::state::boot_identity().unwrap()).unwrap());
}

#[test]
fn conversation_receipts_reject_duplicate_and_symlinked_directories() {
    for defect in ["duplicate", "symlink"] {
        let (tmp, state, binding) = fixture();
        let _fork = crate::os::fork_guard().unwrap();
        let lease = Arc::new(state.runtime_lease(&binding).unwrap());
        let id = Uuid::new_v4().to_string();
        let mut evidence = lease.begin_provider(&id).unwrap();
        evidence.started(std::process::id()).unwrap();
        evidence
            .finish(crate::state::ProcessCompletion {
                quiescent: true,
                confirmed: true,
            })
            .unwrap();
        drop(evidence);
        drop(lease);
        assert!(state.check_conversation(&binding, Some(&id)).is_ok());
        let dir = binding.directory.clone();
        let archive = dir.join("execution-archive");
        if defect == "duplicate" {
            directory(&dir.join("executions")).unwrap();
            fs::copy(
                archive.join(format!("{id}.json")),
                dir.join(format!("executions/{id}.json")),
            )
            .unwrap();
        } else {
            let moved = tmp.path().join("archive");
            fs::rename(&archive, &moved).unwrap();
            std::os::unix::fs::symlink(moved, &archive).unwrap();
        }
        assert!(state.check_conversation(&binding, Some(&id)).is_err());
    }
}
