use super::*;

#[test]
fn missing_ready_ownership_marker_still_reports_recovery_required() {
    let tmp = tempfile::tempdir().unwrap();
    let state = State::new(tmp.path().join("state")).unwrap();
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let binding = state.register("main", &profile).unwrap();
    std::fs::remove_file(binding.directory.join("auth.lock")).unwrap();
    let error = state.runtime_lease(&binding).err().unwrap();
    assert_eq!(error.code(), "recovery_required");
    let source = std::error::Error::source(&error)
        .unwrap()
        .downcast_ref::<Error>()
        .unwrap();
    assert_eq!(source.io_raw_os_error(), Some(libc::ENOENT));
    let mut report = Report::new("main", &profile, None);
    report.fail(error);
    assert_eq!(report.state, Status::RecoveryRequired);
    assert!(
        report
            .diagnostic
            .unwrap()
            .to_string()
            .contains("auth repair --reauthenticate")
    );
    assert!(!binding.directory.join("auth.lock").exists());
}

#[test]
fn ready_generation_readers_preserve_the_original_profile_contention_diagnostic() {
    let tmp = tempfile::tempdir().unwrap();
    let state = State::new(tmp.path().join("state")).unwrap();
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let binding = state.register("main", &profile).unwrap();
    let operation = state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap();
    for error in [
        state.runtime_lease(&binding).err().unwrap(),
        state
            .operation(&binding, crate::state::OperationKind::Status)
            .err()
            .unwrap(),
    ] {
        assert_eq!(error.code(), "profile_busy");
        assert_eq!(
            error.to_string(),
            "profile_busy: An exclusive operation owns this profile."
        );
    }
    operation.finish().unwrap();
}
