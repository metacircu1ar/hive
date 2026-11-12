//! Owned-child signalling and bounded reaping; a signal is never exit evidence.
use crate::Result;
use rustix::process::{self, Pid, Signal, WaitId, WaitIdOptions};
use std::{
    io,
    process::{Child, ExitStatus},
    thread,
    time::{Duration, Instant},
};

pub const FORCED_REAP_GRACE: Duration = Duration::from_secs(2);
pub const LOGIN_INTERRUPT_GRACE: Duration = Duration::from_secs(2);
pub const LOGIN_TERMINATION_GRACE: Duration = Duration::from_secs(5);
const REAP_POLL: Duration = Duration::from_millis(10);

/// Observe without releasing the owned PID, so subsequent signalling cannot hit reuse.
pub fn has_exited(child: &Child) -> Result<bool> {
    observed(child, WaitIdOptions::EXITED)
}
/// A stopped child retains its owned PID; the interactive runner cancels it.
pub fn has_stopped(child: &Child) -> Result<bool> {
    observed(child, WaitIdOptions::STOPPED)
}
fn observed(child: &Child, events: WaitIdOptions) -> Result<bool> {
    loop {
        match process::waitid(
            WaitId::Pid(Pid::from_child(child)),
            // Linux reports ECHILD for a stopped-only query of an exited,
            // unreaped child. Include exits to retain ownership evidence, then
            // classify the event below without consuming it.
            events | WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        ) {
            Ok(status) => {
                // XNU can return a pending stop with WEXITED|WNOWAIT. Presence
                // alone is not exit proof; inspect the native event category.
                return Ok(status.is_some_and(|status| {
                    if events == WaitIdOptions::EXITED {
                        status.exited() || status.killed() || status.dumped()
                    } else {
                        status.stopped() || status.trapped()
                    }
                }));
            }
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

/// None means the deadline elapsed while the child remained live. Ownership errors
/// are returned, not turned into disappearance or permission to signal again.
pub fn reap_until(child: &mut Child, deadline: Instant) -> Result<Option<ExitStatus>> {
    loop {
        if has_exited(child)? {
            return child.wait().map(Some);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        thread::sleep(REAP_POLL.min(remaining));
    }
}

/// Independent errors remain available even when a later observation/reap succeeds.
pub struct KillAttempt {
    pub group_error: Option<io::Error>,
    pub leader_error: Option<io::Error>,
    pub ownership_error: Option<io::Error>,
}
impl KillAttempt {
    pub fn errors(&self) -> impl Iterator<Item = (&'static str, &io::Error)> {
        [
            ("group", &self.group_error),
            ("leader", &self.leader_error),
            ("ownership", &self.ownership_error),
        ]
        .into_iter()
        .filter_map(|(phase, error)| error.as_ref().map(|error| (phase, error)))
    }
}

/// Signal the original group, then the still-owned live leader even if the group
/// signal succeeded: a moved leader can leave a descendant in its original group.
/// The caller must not retry signalling after an ownership error.
pub fn kill_owned(child: &mut Child, group: impl FnOnce(&mut Child) -> Result<()>) -> KillAttempt {
    let mut attempt = KillAttempt {
        group_error: None,
        leader_error: None,
        ownership_error: None,
    };
    if let Err(error) = has_exited(child) {
        attempt.ownership_error = Some(error);
        return attempt;
    }
    attempt.group_error = group(child).err();
    match has_exited(child) {
        Ok(false) => {
            attempt.leader_error = process::kill_process(Pid::from_child(child), Signal::KILL)
                .err()
                .map(Into::into);
        }
        Ok(true) => {}
        Err(error) => attempt.ownership_error = Some(error),
    }
    attempt
}

#[cfg(test)]
mod tests;
