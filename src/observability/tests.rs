use super::*;
use crate::error::Error;

#[test]
fn cleanup_preserves_failure_and_records_both_causes_but_fails_success() {
    let directory = crate::runtime::test_directory();
    let path = directory.path().join("cleanup.jsonl");
    let (backend, guard) = hive_log::start(&path, hive_log::Config::default()).unwrap();
    backend.scope(|| {
        let primary = Error::provider_timeout("Provider call timed out.");
        let cleanup = || {
            Err(Error::state_io_from(std::io::Error::from_raw_os_error(
                libc::EIO,
            )))
        };
        let result: crate::error::Result<()> = settle(Err(primary), cleanup(), "fixture_cleanup");
        assert_eq!(result.unwrap_err().code(), "provider_timeout");
        let result = settle(Ok(17), cleanup(), "fixture_cleanup");
        assert_eq!(result.unwrap_err().io_raw_os_error(), Some(libc::EIO));
    });
    assert_eq!(
        guard.finish(std::time::Duration::from_secs(2)).state,
        hive_log::Shutdown::Drained
    );
    let text = std::fs::read_to_string(path).unwrap();
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["event"], "cleanup.secondary_failed");
    assert_eq!(rows[0]["decision"], "preserve_primary_failure");
    assert!(rows[0]["primary"].is_object());
    assert!(rows[0]["secondary"].is_object());
}
