//! Retain native group ownership until reap and finish the caller's cleanup evidence.
use super::{CAPTURE_POLL_INTERVAL, Child, Duration, Instant, Result, Signal, os};
use crate::{error::Error, state::ChildReceipt};
use std::{result, thread};

pub(crate) enum CleanupReceipt {
    GroupAbsence,
    Unconfirmed,
}

pub(crate) struct OwnedGroup {
    pub(crate) child: Child,
    pub(crate) receipt: ChildReceipt,
    pub(crate) reaped: bool,
    ownership_lost: bool,
    cleanup: CleanupReceipt,
}
impl OwnedGroup {
    pub(crate) fn new(child: Child, receipt: ChildReceipt, cleanup: CleanupReceipt) -> Self {
        Self {
            child,
            receipt,
            reaped: false,
            ownership_lost: false,
            cleanup,
        }
    }
    /// Cleanup continues after a failed signal; retain the error before reap.
    pub(crate) fn signal(&self, signal: Signal) {
        if self.reaped || self.ownership_lost {
            return;
        }
        if let Err(error) = os::signal_group(&self.child, signal) {
            self.signal_error("group", &error, "continue_reap");
        }
    }
    fn signal_error(&self, phase: &'static str, error: &std::io::Error, decision: &'static str) {
        let already_exited = if phase == "group" {
            match signal_already_exited(&self.child, error) {
                Ok(exited) => exited,
                Err(probe) => {
                    tracing::warn!(name: "process.cleanup_probe_failed",
                         pid = self.child.id(), phase = phase,
                         error = &probe as &(dyn std::error::Error + 'static),
                         decision = "retain_signal_warning");
                    false
                }
            }
        } else {
            false
        };
        if already_exited {
            tracing::debug!(name: "process.cleanup_already_exited",
                 pid = self.child.id(), phase = phase,
                 error = error as &(dyn std::error::Error + 'static),
                 decision = "continue_reap");
        } else {
            tracing::warn!(name: "process.cleanup_signal_failed",
                 pid = self.child.id(), phase = phase,
                 error = error as &(dyn std::error::Error + 'static),
                 decision = decision);
        }
    }
    pub(crate) fn exited(&mut self) -> Result<bool> {
        if self.reaped {
            return Ok(true);
        }
        if self.ownership_lost {
            return Err(Error::process_wait_failed(
                "Cannot confirm provider process termination.",
            ));
        }
        os::has_exited(&self.child).map_err(|error| {
            self.ownership_lost = true;
            Error::process_wait_failed("Cannot confirm provider process termination.")
                .with_source(error)
        })
    }
    /// The caller has already observed this owned leader as waitable.
    pub(crate) fn reap_ready(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let result = self.child.wait();
        self.reaped = result.is_ok();
        self.ownership_lost = result.is_err();
        result
    }
    fn force_reap(&mut self) -> bool {
        if self.ownership_lost {
            return false;
        }
        let attempt = hive_setup_support::child::kill_owned(&mut self.child, |child| {
            os::signal_group(child, Signal::KILL)
        });
        for (phase, error) in attempt.errors() {
            self.signal_error(phase, error, "retain_evidence");
        }
        if attempt.ownership_error.is_some() {
            self.ownership_lost = true;
            return false;
        }
        let deadline = Instant::now() + hive_setup_support::child::FORCED_REAP_GRACE;
        match hive_setup_support::child::reap_until(&mut self.child, deadline) {
            Ok(Some(_)) => true,
            Ok(None) => {
                tracing::warn!(name: "process.cleanup_unconfirmed", pid = self.child.id(),
                    code = "reap_timeout", decision = "retain_evidence");
                false
            }
            Err(error) => {
                self.ownership_lost = true;
                tracing::warn!(name: "process.cleanup_wait_failed",
                     pid = self.child.id(), error = &error as &(dyn std::error::Error + 'static),
                     decision = "retain_evidence");
                false
            }
        }
    }
}

/// This classifies a signal error, never a cleanup receipt. The leader remains
/// unreaped and final kernel-proven group absence is still required afterward.
#[cfg_attr(
    not(target_os = "macos"),
    expect(
        clippy::unnecessary_wraps,
        reason = "macOS exit and membership probes can fail."
    )
)]
fn signal_already_exited(child: &Child, error: &std::io::Error) -> std::io::Result<bool> {
    if os::no_such_process(error) {
        return Ok(true);
    }
    #[cfg(target_os = "macos")]
    if error.raw_os_error() == Some(libc::EPERM) && os::has_exited(child)? {
        return crate::os::shell::group_only_leader(child.id());
    }
    #[cfg(not(target_os = "macos"))]
    let _: &Child = child;
    Ok(false)
}
impl Drop for OwnedGroup {
    fn drop(&mut self) {
        if !self.reaped {
            let reaped = self.force_reap();
            self.reaped = reaped;
            // Captured probes may confirm group absence; an interrupted login
            // cannot prove that the provider finished its credential writes.
            let confirmed = match self.cleanup {
                CleanupReceipt::GroupAbsence if reaped => {
                    match wait_group_absent(self.child.id()) {
                        Ok(absent) => absent,
                        Err(error) => {
                            tracing::warn!(name: "process.cleanup_probe_failed",
                             pid = self.child.id(),
                             code = error.code(),
                             error = &error as &(dyn std::error::Error + 'static),
                             decision = "retain_evidence");
                            false
                        }
                    }
                }
                CleanupReceipt::GroupAbsence | CleanupReceipt::Unconfirmed => false,
            };
            if let Err(error) = self.receipt.finish(confirmed) {
                tracing::warn!(name: "process.cleanup_receipt_failed",
                     pid = self.child.id(),
                     code = error.code(),
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "retain_evidence");
            }
        }
    }
}

pub(crate) fn wait_until<E>(
    deadline: Instant,
    interval: Duration,
    mut ready: impl FnMut() -> result::Result<bool, E>,
) -> result::Result<bool, E> {
    loop {
        if ready()? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(interval.min(deadline.saturating_duration_since(Instant::now())));
    }
}

pub(crate) fn wait_group_absent(pid: u32) -> Result<bool> {
    let deadline = Instant::now() + super::GROUP_EXIT_GRACE;
    wait_until(deadline, CAPTURE_POLL_INTERVAL, || {
        // An adopted member that exited stays in the group until reaped.
        os::reap_group(pid).map_err(Error::state_io_from)?;
        os::group_absent(pid).map_err(Error::state_io_from)
    })
}

#[cfg(test)]
mod tests;
