use super::*;
use std::io;

#[test]
fn native_sandbox_failures_preserve_causes_without_logging_private_text() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("diagnostics.jsonl");
    let (backend, guard) = hive_log::start(&path, hive_log::Config::default()).unwrap();
    backend.scope(|| {
        let secret = || io::Error::new(io::ErrorKind::PermissionDenied, "secret-probe-canary");
        let report = failed(Path::new("/provider"), &secret());
        assert_eq!(report.code, "probe_io_failed");
        let report = failed(Path::new("/provider"), &io::ErrorKind::TimedOut.into());
        assert_eq!(report.code, "timed_out");
        let binary = fixture(
            directory.path(),
            "echo secret-probe-canary; echo 'secret-probe-canary: Operation not permitted' >&2; exit 23",
        );
        assert_eq!(execute(&binary, TIMEOUT).unwrap().code, "sandbox_permission_denied");
        let result = finish_probe(Err(io::Error::from_raw_os_error(libc::EIO)), Err(secret()));
        assert_eq!(result.unwrap_err().raw_os_error(), Some(libc::EIO));
        let report = Report::new(Outcome::Failed, None, "sandbox_permission_denied");
        let result = finish_probe(Ok(report), Err(secret())).unwrap();
        assert_eq!(result.code, "sandbox_permission_denied");
        let report = Report::new(Outcome::Passed, None, "native_command_completed");
        let result = finish_probe(Ok(report), Err(secret())).unwrap();
        assert_eq!(result.outcome, Outcome::Failed);
        assert_eq!(result.code, "probe_cleanup_failed");
    });
    assert_eq!(
        guard.finish(Duration::from_secs(2)).state,
        hive_log::Shutdown::Drained
    );
    let text = fs::read_to_string(path).unwrap();
    assert!(!text.contains("secret-probe-canary"));
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let failures: Vec<_> = rows
        .iter()
        .filter(|row| row["event"] == "sandbox.probe_io_failed")
        .collect();
    assert_eq!(failures.len(), 2);
    assert_eq!(
        failures[0]["error"]["items"][0]["io_kind"],
        "PermissionDenied"
    );
    assert_eq!(failures[1]["error"]["items"][0]["io_kind"], "TimedOut");
    let failures: Vec<_> = rows
        .iter()
        .filter(|row| row["event"] == "sandbox.native_command_failed")
        .collect();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0]["exit_code"], 23);
    assert_eq!(failures[0]["code"], "sandbox_permission_denied");
    assert_eq!(
        rows.iter()
            .filter(|row| row["event"] == "sandbox.probe_cleanup_failed")
            .count(),
        3
    );
}
