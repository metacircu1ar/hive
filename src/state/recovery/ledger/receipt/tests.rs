use super::*;
use crate::{
    config::{Method, Profile, Provider},
    state::{ChildAccess, OperationKind, State},
};
use atomic_fs::Step;

fn registered() -> (tempfile::TempDir, State, crate::state::Binding) {
    let tmp = tempfile::tempdir().unwrap();
    let state = State::new(tmp.path().join("state")).unwrap();
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let binding = state.register("main", &profile).unwrap();
    (tmp, state, binding)
}

#[test]
fn published_update_failure_advances_only_the_visible_revision_and_can_finish() {
    for (at, revision) in [(Step::BeforePublish, 0), (Step::BeforeDirectorySync, 1)] {
        let (_tmp, state, binding) = registered();
        let _fork = crate::os::fork_guard().unwrap();
        let lease = Arc::new(state.runtime_lease(&binding).unwrap());
        let mut evidence = lease.begin_provider(&Uuid::new_v4().to_string()).unwrap();
        let path = evidence.receipt.path.clone();
        let target = path.clone();
        let error = crate::state::sync_tests::with_hook(
            move |path, step| {
                if path == target && step == at {
                    Err(std::io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || evidence.bind_conversation("agent", "execution", "thread"),
        )
        .unwrap_err();
        assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
        assert_eq!(evidence.receipt.record.lock().unwrap().revision, revision);
        assert_eq!(read_json::<Record>(&path).unwrap().revision, revision);
        evidence
            .finish(crate::state::ProcessCompletion {
                quiescent: true,
                confirmed: true,
            })
            .unwrap();
        assert!(!path.exists());
    }
}

#[test]
fn failed_child_authorization_can_still_record_definitely_unstarted_cleanup() {
    for at in [Step::BeforePublish, Step::BeforeDirectorySync] {
        let (_tmp, state, binding) = registered();
        let _fork = crate::os::fork_guard().unwrap();
        let mut operation = state.operation(&binding, OperationKind::Login).unwrap();
        let child = operation
            .prepare_child(true, ChildAccess::CredentialWriter)
            .unwrap();
        let error = crate::state::sync_tests::with_hook(
            move |path, step| {
                if path
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|name| name == "writers")
                    && step == at
                {
                    Err(std::io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || child.started(std::process::id()),
        )
        .unwrap_err();
        assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
        child.not_started().unwrap();
        assert!(operation.children_confirmed());
    }
}
