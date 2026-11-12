//! Process-lifetime child ownership for applications which also use async signals.
use std::io;

/// Keep native child exit statuses available until all application children end.
/// Admit this on the main thread before spawning workers. Nested synchronous
/// `Signals` scopes restore this default disposition when they finish.
pub struct Reaping(Option<libc::sigaction>);

impl Reaping {
    pub fn new() -> io::Result<Self> {
        // SAFETY: both C records are initialized, including the signal mask.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = libc::SIG_DFL;
        // SAFETY: initialize native output storage before sigaction writes it.
        let mut previous = unsafe { std::mem::zeroed() };
        // SAFETY: live initialized records; flags=0 disables SA_NOCLDWAIT too.
        let failed = unsafe {
            libc::sigemptyset(&raw mut action.sa_mask) != 0
                || libc::sigaction(libc::SIGCHLD, &raw const action, &raw mut previous) != 0
        };
        if failed {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(Some(previous)))
    }

    pub fn restore(&mut self) -> io::Result<()> {
        if let Some(previous) = self.0.take() {
            // SAFETY: this is the exact saved action for SIGCHLD.
            if unsafe { libc::sigaction(libc::SIGCHLD, &raw const previous, std::ptr::null_mut()) }
                != 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

impl Drop for Reaping {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            tracing::error!(name: "setup.signal_restore_failed",
                error = &error as &(dyn std::error::Error + 'static),
                decision = "preserve_primary_failure");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::Command,
        time::{Duration, Instant},
    };

    #[test]
    #[ignore = "native fixture invoked in an isolated subprocess"]
    fn reaping_fixture_child() {
        let mode = std::env::var("HIVE_REAPING_FIXTURE").unwrap();
        // SAFETY: isolated fixture process; initialized native action and mask.
        let mut inherited: libc::sigaction = unsafe { std::mem::zeroed() };
        inherited.sa_sigaction = if mode == "ignore" {
            libc::SIG_IGN
        } else {
            libc::SIG_DFL
        };
        inherited.sa_flags = if mode == "no-cldwait" {
            libc::SA_NOCLDWAIT
        } else {
            0
        };
        // SAFETY: live initialized records; no other fixture threads own children.
        unsafe {
            assert_eq!(libc::sigemptyset(&raw mut inherited.sa_mask), 0);
            assert_eq!(
                libc::sigaction(libc::SIGCHLD, &raw const inherited, std::ptr::null_mut()),
                0
            );
        }
        // SIG_IGN implies SA_NOCLDWAIT on XNU. Compare the actual native
        // disposition saved by the guard, not the input flags before normalization.
        // SAFETY: initialized native output record for a read-only query.
        let mut expected = unsafe { std::mem::zeroed() };
        assert_eq!(
            // SAFETY: null input queries into live initialized output storage.
            unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &raw mut expected) },
            0
        );
        let mut reaping = Reaping::new().unwrap();
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let status = crate::child::reap_until(&mut child, Instant::now() + Duration::from_secs(3))
            .unwrap()
            .unwrap();
        assert!(status.success());
        reaping.restore().unwrap();
        // SAFETY: query into initialized local output storage.
        let mut restored = unsafe { std::mem::zeroed() };
        assert_eq!(
            // SAFETY: null input queries the current action without changing it.
            unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &raw mut restored) },
            0
        );
        assert_eq!(restored.sa_sigaction, expected.sa_sigaction);
        assert_eq!(
            restored.sa_flags & libc::SA_NOCLDWAIT,
            expected.sa_flags & libc::SA_NOCLDWAIT
        );
    }

    #[test]
    fn inherited_auto_reaping_is_disabled_and_restored() {
        for mode in ["ignore", "no-cldwait"] {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--ignored",
                    "--exact",
                    "runner::reaping::tests::reaping_fixture_child",
                ])
                .env("HIVE_REAPING_FIXTURE", mode);
            let output = crate::runner::capture_command(
                Command::spawn,
                &mut command,
                Duration::from_secs(10),
                &|| false,
            )
            .unwrap();
            assert_eq!(output.code, 0, "{} {}", output.stdout, output.stderr);
        }
    }
}
