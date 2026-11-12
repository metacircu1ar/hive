use super::*;
use std::{
    ffi::OsString,
    fs,
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    time::Instant,
};
mod logging;

#[test]
fn native_sandbox_home_is_private_and_outside_system_temp() {
    let directory = probe_directory().unwrap();
    let path = directory.path().canonicalize().unwrap();
    assert!(!path.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
    directory.close().unwrap();
    assert!(!path.exists());
}

#[test]
fn native_sandbox_report_handles_non_utf8_executable_paths() {
    let path = PathBuf::from(OsString::from_vec(b"/provider/codex-\xff".to_vec()));
    for outcome in [
        Outcome::Passed,
        Outcome::Failed,
        Outcome::Busy,
        Outcome::Unsupported,
    ] {
        let report = Report::new(outcome, Some(path.clone()), "test");
        let value = crate::api::value(report);
        assert_eq!(value["executable"], path.to_string_lossy().as_ref());
        let decoded: Report = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.outcome, outcome);
    }
}

fn fixture(directory: &Path, body: &str) -> PathBuf {
    let path = directory.join("codex");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[test]
fn native_sandbox_probe_has_no_auth_and_runs_only_the_fixed_command() {
    let dir = tempfile::tempdir().unwrap();
    let binary = fixture(
        dir.path(),
        r#"
test "$HOME" = "$CODEX_HOME" || exit 1
test "$PWD" = "$HOME" || exit 2
test ! -e "$HOME/auth.json" || exit 3
test ! -e "$HOME/config.toml" || exit 4
test -z "$OPENAI_API_KEY$ANTHROPIC_API_KEY" || exit 5
test "$1" = -c && test "$2" = 'cli_auth_credentials_store="file"' || exit 6
shift 2
test "$1" = -c && test "$2" = 'sandbox_mode="read-only"' || exit 7
shift 2
test "$1" = sandbox && test "$2" = -- || exit 8
shift 2
test "$#" = 2 && test "$1" = /bin/echo || exit 9
exec "$@"
"#,
    );
    let report = execute(&binary, TIMEOUT).unwrap();
    assert_eq!(report.outcome, Outcome::Passed);
}

#[test]
fn native_sandbox_probe_rejects_false_success_and_sanitizes_errors() {
    let dir = tempfile::tempdir().unwrap();
    for (body, code) in [
        ("exit 0", "native_command_failed"),
        ("echo secret-canary; exit 1", "native_command_failed"),
        (
            "echo 'secret-canary: Operation not permitted' >&2; exit 1",
            "sandbox_permission_denied",
        ),
    ] {
        let report = execute(&fixture(dir.path(), body), TIMEOUT).unwrap();
        assert_eq!(report.outcome, Outcome::Failed);
        assert_eq!(report.code, code);
        assert!(
            !serde_json::to_string(&report)
                .unwrap()
                .contains("secret-canary")
        );
    }
}

#[test]
fn native_sandbox_probe_bounds_time_and_output() {
    let dir = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let error = execute(
        &fixture(dir.path(), "sleep 30 & wait"),
        Duration::from_millis(100),
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(start.elapsed() < Duration::from_secs(3));
    let error = execute(&fixture(dir.path(), "yes secret-canary"), TIMEOUT).unwrap_err();
    assert_ne!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[test]
fn native_sandbox_probe_is_not_required_for_claude_only() {
    for (provider, expected) in [("claude", false), ("codex", true)] {
        let config = Config::parse(&format!(
            "auth_profiles: {{main: {{provider: {provider}, method: subscription}}}}\ndefault_provider: {provider}\nproviders: {{{provider}: {{model: default}}}}\n"
        )).unwrap();
        assert_eq!(configured(&config), expected);
    }
}
