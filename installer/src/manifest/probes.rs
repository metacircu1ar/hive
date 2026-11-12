//! Bound native prerequisite commands and reject failure before parsing output.
use crate::Result;
use hive_setup_support::runner;
use std::{process::Command, time::Duration};
#[cfg(test)]
mod cancellation_tests;

pub(super) const TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn run(command: &mut Command, timeout: Duration) -> Result<String> {
    let cancellation = crate::cancellation::Cancellation::new()?;
    cancellation.configure_child(command);
    let output = runner::capture_command(Command::spawn, command, timeout, &|| {
        cancellation.cancelled()
    })
    .map_err(Into::into);
    let output = cancellation.finish(output)?;
    if output.code != 0 {
        return Err(format!(
            "Release prerequisite command failed (exit code {}).",
            output.code
        )
        .into());
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn failed_status_refuses_parseable_output_without_exposing_native_text() {
        let error = run(
            Command::new("/bin/sh").args([
                "-c",
                "printf 'glibc 2.35'; printf 'PRIVATE_NATIVE_FAILURE' >&2; exit 1",
            ]),
            TIMEOUT,
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("exit code 1"));
        assert!(!message.contains("PRIVATE_NATIVE_FAILURE"));
        assert!(!message.contains("glibc"));
    }

    #[test]
    fn prerequisite_hang_and_flood_are_bounded() {
        let start = Instant::now();
        let error = run(
            Command::new("/bin/sh").args(["-c", "sleep 60"]),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert_eq!(
            error
                .as_ref()
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .kind(),
            std::io::ErrorKind::TimedOut
        );
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(
            run(
                Command::new("/bin/sh").args(["-c", "while :; do printf abcdefghijklmnop; done"]),
                TIMEOUT
            )
            .is_err()
        );
    }
}
