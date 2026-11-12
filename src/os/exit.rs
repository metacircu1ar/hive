//! POSIX process status conversion shared with the trusted launcher.
pub(crate) const EXEC_FAILED_EXIT: i32 = 126;

/// POSIX process exit statuses retain the low byte, including shell signal codes.
pub(crate) fn exit_code(status: i32) -> std::process::ExitCode {
    std::process::ExitCode::from(status.to_le_bytes()[0])
}

#[cfg(test)]
#[test]
fn exit_codes_preserve_posix_status_and_shell_signal_values() {
    for (status, expected) in [
        (0, 0),
        (2, 2),
        (126, 126),
        (130, 130),
        (255, 255),
        (-1, 255),
        (256, 0),
    ] {
        assert_eq!(exit_code(status), std::process::ExitCode::from(expected));
    }
}
