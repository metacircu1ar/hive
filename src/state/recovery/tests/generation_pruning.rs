//! Generation pruning regressions.
use super::*;

#[test]
fn pruning_without_intent_cannot_repair_missing_content_directories() {
    let (_tmp, state, binding) = fixture();
    // This test has no child processes. Keep unrelated test forks from briefly
    // retaining a just-released credential OFD before their close-on-exec runs.
    let _fork = crate::os::fork_guard().unwrap();
    let repair = state.begin_repair("main", &binding.profile).unwrap();
    state.finish_repair(&repair, true, None).unwrap();
    crate::state::test_support::edit_index(&state, |index| {
        index.generations.get_mut(&binding.id).unwrap().state = GenerationState::Pruning;
    });
    fs::remove_dir_all(binding.directory.join("provider")).unwrap();
    // Maintenance admission creates this stable lock before checking credential trees.
    drop(crate::state::daemon::maintenance_lock(&state.root).unwrap());
    let before = snapshot_files(&state.root);
    assert!(state.prune("main", binding.id).is_err());
    assert_eq!(snapshot_files(&state.root), before);
    assert!(!binding.directory.join("provider").exists());
    assert!(!binding.directory.join("prune-receipt.json").exists());
}

#[test]
fn abandoned_candidate_is_prunable_but_active_repair_is_not() {
    for ready in [true, false] {
        let (_tmp, state, binding) = fixture();
        let repair = state.begin_repair("main", &binding.profile).unwrap();
        assert_eq!(
            state.prune("main", repair.binding.id).unwrap_err().code(),
            "profile_busy"
        );
        let mut child = crate::test_support::ChildGuard(
            crate::os::spawn(std::process::Command::new("/bin/sleep").arg("10")).unwrap(),
        );
        let owner = ProcessIdentity::read(child.id()).unwrap().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        crate::state::test_support::edit_index(&state, |index| {
            let generation = index.generations.get_mut(&repair.binding.id).unwrap();
            generation.repair_owner = Some(owner);
            if !ready {
                generation.binding.ready = false;
                generation.credential_lock = None;
                let dir = repair.binding.directory.clone();
                fs::remove_file(dir.join("auth.lock")).unwrap();
                write_json(&dir.join("identity.json"), &generation.binding).unwrap();
            }
        });
        state.prune("main", repair.binding.id).unwrap();
        assert_eq!(
            state.lookup("main", &binding.profile).unwrap().unwrap().id,
            binding.id
        );
    }
}

#[test]
fn dead_probe_allows_in_place_login_and_prune_without_conversation_proof() {
    use std::os::unix::process::CommandExt;
    let (_tmp, state, binding) = fixture();
    let mut operation = state
        .operation(&binding, crate::state::OperationKind::Status)
        .unwrap();
    let receipt = operation
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
    receipt.started(child.id()).unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    // The child is reaped; exclude unrelated test forks while dropping the old
    // credential descriptor and immediately asserting exclusive reacquisition.
    let _fork = crate::os::fork_guard().unwrap();
    receipt.finish(false).unwrap();
    drop(receipt);
    operation.finish().unwrap();
    state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap()
        .finish()
        .unwrap();
    let replacement = state.begin_repair("main", &binding.profile).unwrap();
    state.finish_repair(&replacement, true, None).unwrap();
    state.prune("main", binding.id).unwrap();
}

#[test]
fn retired_account_fence_does_not_prevent_proven_local_credential_pruning() {
    let (_tmp, state, binding) = fixture();
    state.record_account(&binding, Some("old".into())).unwrap();
    let mut op = state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap();
    let writer = op
        .prepare_child(true, crate::state::ChildAccess::CredentialWriter)
        .unwrap();
    writer.started(std::process::id()).unwrap();
    writer.finish(true).unwrap();
    complete_reconciliation_probe(&mut op);
    assert!(op.confirm_account(Some("changed".into())).is_err());
    drop(writer);
    drop(op);
    let evidence = binding.directory.join("execution-archive");
    let before: Vec<_> = fs::read_dir(&evidence)
        .unwrap()
        .map(|p| fs::read(p.unwrap().path()).unwrap())
        .collect();
    let repair = state.begin_repair("main", &binding.profile).unwrap();
    state
        .finish_repair(&repair, true, Some("new".into()))
        .unwrap();
    let report = state.prune("main", binding.id).unwrap();
    assert_eq!(report["outcome"], "pruned");
    assert!(!before.is_empty());
    let after: Vec<_> = fs::read_dir(evidence)
        .unwrap()
        .map(|p| fs::read(p.unwrap().path()).unwrap())
        .collect();
    assert_eq!(before, after);
}

#[test]
fn pruning_refuses_dangling_symlinks_and_empty_databases_but_accepts_current_or_absent() {
    for defect in ["runtime", "receipt", "empty", "current", "absent"] {
        let (tmp, state, binding) = fixture();
        let _fork = crate::os::fork_guard().unwrap();
        let sentinel = binding.directory.join("provider/sentinel");
        fs::write(&sentinel, "retained").unwrap();
        let repair = state.begin_repair("main", &binding.profile).unwrap();
        state.finish_repair(&repair, true, None).unwrap();
        directory(&state.root.join("runtime")).unwrap();
        let path = if defect == "receipt" {
            binding.directory.join("prune-receipt.json")
        } else {
            state.root.join("runtime/runtime.sqlite")
        };
        if matches!(defect, "empty" | "current") {
            crate::state::private_file(&path, true).unwrap();
            if defect == "current" {
                let connection = rusqlite::Connection::open(&path).unwrap();
                crate::state::database::schema::initialize(&connection).unwrap();
            }
        } else if defect != "absent" {
            std::os::unix::fs::symlink(tmp.path().join("absent"), &path).unwrap();
        }
        if matches!(defect, "current" | "absent") {
            state.prune("main", binding.id).unwrap();
            assert!(!sentinel.exists());
        } else {
            let error = state.prune("main", binding.id).unwrap_err();
            if defect == "empty" {
                assert_eq!(error.code(), "unsupported_schema");
                assert!(fs::read(&path).unwrap().is_empty());
            }
            assert_eq!(fs::read_to_string(sentinel).unwrap(), "retained");
        }
    }
}
