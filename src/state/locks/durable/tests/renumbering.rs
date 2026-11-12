//! Saved device numbers may change across mounts; live inode checks remain strict.
use super::*;
use crate::state::OperationKind;

fn change_saved_lock(state: &State, binding: &Binding, field: &str) {
    edit_index(state, |index| {
        let generation = index.generations.get_mut(&binding.id).unwrap();
        let mut value = serde_json::to_value(generation.credential_lock.as_ref().unwrap()).unwrap();
        value[field] = serde_json::json!(value[field].as_u64().unwrap() ^ 1);
        generation.credential_lock = Some(serde_json::from_value(value).unwrap());
    });
}

#[test]
fn saved_device_renumbering_keeps_ready_credentials_usable_without_rewriting_pins() {
    let (_temporary, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    change_saved_lock(&state, &binding, "dev");
    let before = fs::read(state.root.join("index.json")).unwrap();
    {
        let lease = state.runtime_lease(&binding).unwrap();
        lease.verify(&state, &binding).unwrap();
        drop(state.operation(&binding, OperationKind::Status).unwrap());
    }
    assert_eq!(state.upgrade(true).unwrap().status, "current");
    assert_eq!(state.upgrade(false).unwrap().status, "current");
    assert_eq!(fs::read(state.root.join("index.json")).unwrap(), before);
}

#[test]
fn changed_saved_inode_never_authorizes_a_ready_generation() {
    let (_temporary, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    change_saved_lock(&state, &binding, "ino");
    assert!(state.runtime_lease(&binding).is_err());
    assert!(state.operation(&binding, OperationKind::Status).is_err());
    assert!(state.upgrade(true).is_err());
}

#[test]
fn saved_receipts_allow_renumbering_only_after_their_boot_ends() {
    for (cross_boot, field, allowed) in [
        (true, "dev", true),
        (false, "dev", false),
        (true, "ino", false),
    ] {
        let (_temporary, state, binding) = fixture();
        let _fork = crate::os::fork_guard().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let lease = std::sync::Arc::new(state.runtime_lease(&binding).unwrap());
        let mut receipt = lease.begin_provider(&id).unwrap();
        receipt
            .finish(crate::state::ProcessCompletion {
                quiescent: true,
                confirmed: true,
            })
            .unwrap();
        drop(receipt);
        drop(lease);
        let path = binding
            .directory
            .join(format!("execution-archive/{id}.json"));
        let mut value: serde_json::Value = crate::state::read_json(&path).unwrap();
        value["lock"][field] = serde_json::json!(value["lock"][field].as_u64().unwrap() ^ 1);
        if cross_boot {
            value["boot"] = uuid::Uuid::nil().to_string().into();
        }
        crate::state::write_json(&path, &value).unwrap();
        let before = fs::read(&path).unwrap();
        assert_eq!(
            state.check_conversation(&binding, Some(&id)).is_ok(),
            allowed
        );
        assert_eq!(state.upgrade(true).is_ok(), allowed);
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn renumbered_saved_pin_never_hides_a_replaced_live_credential_lock() {
    let (_temporary, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let lease = state.runtime_lease(&binding).unwrap();
    change_saved_lock(&state, &binding, "dev");
    let path = binding.directory.join("auth.lock");
    fs::rename(&path, binding.directory.join("retained.lock")).unwrap();
    drop(crate::state::private_file(&path, true).unwrap());
    assert!(lease.verify(&state, &binding).is_err());
    assert!(state.runtime_lease(&binding).is_err());
}

#[test]
fn pending_writer_reconciliation_requires_exact_device_on_the_current_boot() {
    for cross_boot in [false, true] {
        let (_temporary, state, binding) = fixture();
        let _fork = crate::os::fork_guard().unwrap();
        let mut operation = state.operation(&binding, OperationKind::Login).unwrap();
        let child = operation
            .prepare_child(true, crate::state::ChildAccess::CredentialWriter)
            .unwrap();
        child.started(std::process::id()).unwrap();
        child.finish(true).unwrap();
        drop(child);
        drop(operation);
        let pending = state
            .pending_credential_attempt(binding.id)
            .unwrap()
            .unwrap();
        let path = binding
            .directory
            .join(format!("execution-archive/{}.json", pending.writer));
        let mut value: serde_json::Value = crate::state::read_json(&path).unwrap();
        value["lock"]["dev"] = serde_json::json!(value["lock"]["dev"].as_u64().unwrap() ^ 1);
        if cross_boot {
            value["boot"] = uuid::Uuid::nil().to_string().into();
        }
        crate::state::write_json(&path, &value).unwrap();
        let before = fs::read(&path).unwrap();
        let result = state.operation(&binding, OperationKind::Recover);
        assert_eq!(result.is_ok(), cross_boot);
        drop(result);
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
