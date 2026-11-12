//! Let setup restore modes and tear down its own separately grouped children.
use super::{CAPTURE_POLL, Guard, Ownership};
use crate::{Result, invalid};
use rustix::process::{self, Pid, Signal};
use std::{
    io::{self, Read},
    process::{ChildStderr, ChildStdout},
    time::{Duration, Instant},
};
const GRACE: Duration = crate::child::LOGIN_INTERRUPT_GRACE
    .saturating_add(crate::child::LOGIN_TERMINATION_GRACE)
    .saturating_add(crate::child::FORCED_REAP_GRACE)
    .saturating_add(Duration::from_secs(3));

pub(super) fn graceful(
    owner: &mut Guard,
    stdout: &mut Option<ChildStdout>,
    stderr: &mut Option<ChildStderr>,
) -> Result<()> {
    if owner.1 != Ownership::Owned {
        return Err(invalid("Setup child ownership is uncertain."));
    }
    // Notify nested owners in the same group too (setup can be synchronously
    // waiting on `hivectl auth login`). Avoid a duplicate signal to an in-group owner.
    signal_group(owner, Signal::TERM)?;
    signal_group(owner, Signal::CONT)?;
    let deadline = Instant::now() + GRACE;
    while !owner.exited()? && Instant::now() < deadline {
        // Drain bounded chunks without retaining unbounded output on an already
        // failed call, so cleanup cannot stall on a full captured-output pipe.
        drain(stdout, "stdout");
        drain(stderr, "stderr");
        std::thread::sleep(CAPTURE_POLL);
    }
    // The caller retains the leader unreaped and always runs its forced guard.
    Ok(())
}
fn drain(stream: &mut Option<impl Read>, phase: &'static str) {
    if let Some(input) = stream
        && let Err(error) = discard(input)
    {
        tracing::error!(name: "setup.child_cancel_failed", phase = phase,
            error = &error as &(dyn std::error::Error + 'static),
            decision = "preserve_primary_failure");
        // A broken capture stream must not skip the owner's cleanup grace.
        *stream = None;
    }
}
fn signal_group(owner: &mut Guard, signal: Signal) -> Result<()> {
    if owner.exited()? {
        return Ok(());
    }
    let leader = Pid::from_child(&owner.0);
    let outside = match process::getpgid(Some(leader)) {
        Ok(group) => group != leader,
        Err(rustix::io::Errno::SRCH) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let group = process::kill_process_group(leader, signal);
    if let Err(error) = &group
        && *error != rustix::io::Errno::SRCH
    {
        tracing::error!(name: "setup.child_cancel_failed", phase = "signal_group",
            error = &io::Error::from(*error) as &(dyn std::error::Error + 'static),
            decision = "preserve_primary_failure");
    }
    if (outside || group.is_err()) && !owner.exited()? {
        match process::kill_process(leader, signal) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
fn discard(stream: &mut impl Read) -> Result<()> {
    let mut buffer = [0; 4096];
    for _ in 0..16 {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
