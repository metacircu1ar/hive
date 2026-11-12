//! macOS can refuse a signal when the only group member is an owned zombie.
use crate::Result;
use crate::child::has_exited as exited;
use rustix::process::Pid;
use std::{io, mem, process::Child};

pub(super) fn exited_alone(child: &Child) -> bool {
    match inspect(child) {
        Ok(confirmed) => confirmed,
        Err(error) => {
            tracing::warn!(name: "setup.child_observation_failed",
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "preserve_signal_failure");
            false
        }
    }
}

fn inspect(child: &Child) -> Result<bool> {
    if !exited(child)? {
        return Ok(false);
    }
    // Two slots distinguish an exact one-member snapshot from a full buffer.
    // The unreaped child retains the PGID through this proof and the signal.
    let mut members = [0_i32; 2];
    let bytes = i32::try_from(mem::size_of_val(&members)).map_err(io::Error::other)?;
    // SAFETY: PROC_PGRP_ONLY (2) writes PIDs into the sized output buffer.
    let count = unsafe { libc::proc_listpids(2, child.id(), members.as_mut_ptr().cast(), bytes) };
    if count <= 0 {
        let error = io::Error::last_os_error();
        return Err(if error.raw_os_error() == Some(0) {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Process group enumeration was empty",
            )
        } else {
            error
        });
    }
    Ok(count == bytes / 2 && members[0] == Pid::from_child(child).as_raw_pid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{Guard, Ownership};
    use std::{
        os::unix::process::CommandExt,
        process::Command,
        time::{Duration, Instant},
    };

    fn child(body: &str) -> Result<Guard> {
        Ok(Guard(
            Command::new("/bin/sh")
                .args(["-c", body])
                .process_group(0)
                .spawn()?,
            Ownership::Owned,
        ))
    }

    fn wait_for_exit(child: &Child) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !exited(child)? {
            if Instant::now() >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }

    #[test]
    fn exited_owned_leader_preserves_nonzero_status() -> Result<()> {
        let mut child = child("exit 73")?;
        wait_for_exit(&child.0)?;
        assert!(inspect(&child.0)?);
        child.kill_group()?;
        assert_eq!(child.wait_ready()?.code(), Some(73));
        Ok(())
    }

    #[test]
    fn live_leader_and_other_members_never_prove_zombie_only_cleanup() -> Result<()> {
        let running = child("sleep 30")?;
        assert!(!inspect(&running.0)?);
        let mut parent = child("sleep 30 & exit 0")?;
        wait_for_exit(&parent.0)?;
        assert!(!inspect(&parent.0)?);
        parent.kill_group()?;
        assert_eq!(parent.wait_ready()?.code(), Some(0));
        Ok(())
    }
}
