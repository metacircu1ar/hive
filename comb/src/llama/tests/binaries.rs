use super::*;

#[test]
fn user_binary_checks_name_the_failed_step() {
    let directory = tempfile::tempdir().unwrap();
    let pin = Pin::parse(&pin_text(&"d".repeat(40), &"f".repeat(64), 5)).unwrap();
    let missing = Binary::Path(directory.path().join("absent"));
    assert_eq!(
        check_binary(&missing, &pin, &|| false).unwrap_err().message,
        crate::Error::llama_server_not_found().message
    );
    let plain = directory.path().join("plain");
    std::fs::write(&plain, "not a program").unwrap();
    assert_eq!(
        check_binary(&Binary::Path(plain), &pin, &|| false)
            .unwrap_err()
            .message,
        crate::Error::llama_server_not_executable().message
    );
    let failing = directory.path().join("failing");
    std::fs::write(&failing, "#!/bin/sh\nexit 4\n").unwrap();
    std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        check_binary(&Binary::Path(failing), &pin, &|| false)
            .unwrap_err()
            .message,
        crate::Error::llama_server_version_failed().message
    );
}

#[test]
fn an_untested_version_is_a_warning_and_the_pinned_one_is_tested() {
    let directory = tempfile::tempdir().unwrap();
    let pin = Pin::parse(&pin_text(&"d".repeat(40), &"f".repeat(64), 5)).unwrap();
    let tested = fake_server(directory.path(), &COMMIT[..7]);
    let checked = check_binary(&Binary::Path(tested.clone()), &pin, &|| false).unwrap();
    assert!(checked.tested);
    assert_eq!(checked.device, Device::Own);
    assert_eq!(checked.program, tested);
    let other = tempfile::tempdir().unwrap();
    let untested = fake_server(other.path(), "fedcba9");
    assert!(
        !check_binary(&Binary::Path(untested), &pin, &|| false)
            .unwrap()
            .tested
    );
}

#[test]
fn a_missing_bundled_server_names_the_reinstall_or_user_binary_choice() {
    // Test executables never have comb-llama-server beside them.
    let pin = Pin::parse(&pin_text(&"d".repeat(40), &"f".repeat(64), 5)).unwrap();
    assert_eq!(
        check_binary(&Binary::Bundled, &pin, &|| false)
            .unwrap_err()
            .message,
        crate::Error::llama_server_missing().message
    );
}

#[test]
fn cancellation_stops_a_hanging_version_probe_before_its_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let program = directory.path().join("slow-version");
    std::fs::write(&program, "#!/bin/sh\nexec sleep 60\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let pin = pin_for(b"model");
    let start = Instant::now();
    let error = check_binary(&Binary::Path(program), &pin, &|| {
        start.elapsed() > std::time::Duration::from_millis(100)
    })
    .unwrap_err();
    assert_eq!(
        error.message,
        crate::Error::llama_server_version_failed().message
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    let cause = std::error::Error::source(&error).unwrap();
    assert_eq!(
        cause.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::Interrupted
    );
}
