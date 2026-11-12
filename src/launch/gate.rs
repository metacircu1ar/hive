//! Spawn trusted launchers and grant native execution after durable authorization.
use super::{LauncherCache, STATUS_BYTES, STATUS_READ_BYTES, STATUS_TAG, TAG_BYTES};
use crate::{
    error::{Error, Result},
    os,
};
use std::{
    fs,
    io::{self, ErrorKind, Read, Write},
    os::{
        fd::{AsFd, AsRawFd},
        unix::net::UnixStream,
    },
    process::{Child, Command, Stdio},
    time::Instant,
};

pub(crate) fn spawn(
    launcher: &LauncherCache,
    original: &Command,
    group: bool,
    inherited: &[i32],
    authorize: impl FnOnce(u32) -> Result<()>,
) -> Result<Child> {
    spawn_with_terminal(
        launcher,
        original,
        crate::launch::SpawnIo {
            group,
            inherited,
            terminal: None,
        },
        authorize,
    )
}
pub(crate) struct SpawnIo<'a> {
    pub group: bool,
    pub inherited: &'a [i32],
    pub terminal: Option<&'a fs::File>,
}
pub(crate) fn spawn_with_terminal(
    launcher: &LauncherCache,
    original: &Command,
    io: SpawnIo<'_>,
    authorize: impl FnOnce(u32) -> Result<()>,
) -> Result<Child> {
    let SpawnIo {
        group,
        inherited,
        terminal,
    } = io;
    let PreparedSpawn {
        child,
        parent,
        deadline,
    } = prepare_spawn(launcher, original, group, inherited, terminal).map_err(|cause| {
        if matches!(cause, Error::SpawnFailed { .. }) {
            return cause;
        }
        Error::spawn_failed("Cannot prepare the trusted provider launcher.").with_source(cause)
    })?;
    complete_launch(child, parent, deadline, authorize)
}
pub(super) struct PreparedSpawn {
    pub(super) child: Child,
    pub(super) parent: UnixStream,
    pub(super) deadline: Instant,
}
pub(super) fn prepare_spawn(
    launcher: &LauncherCache,
    original: &Command,
    group: bool,
    inherited: &[i32],
    terminal: Option<&fs::File>,
) -> Result<PreparedSpawn> {
    os::reset_sigchld().map_err(Error::state_io_from)?;
    let _fork = os::fork_guard().map_err(Error::state_io_from)?;
    let (parent, child_gate) = UnixStream::pair().map_err(Error::state_io_from)?;
    os::nonblocking(&parent).map_err(Error::state_io_from)?;
    let image = launcher.image()?;
    #[cfg(test)]
    crate::state::sync_tests::step(&image, atomic_fs::Step::BeforePublish)
        .map_err(Error::state_io_from)?;
    let mut command = Command::new(&image);
    let mut fds = inherited.to_vec();
    fds.push(child_gate.as_raw_fd());
    command
        .arg(child_gate.as_raw_fd().to_string())
        .arg(fds.iter().map(i32::to_string).collect::<Vec<_>>().join(","))
        .arg(original.get_program())
        .args(original.get_args())
        .env_clear();
    for (key, value) in original.get_envs() {
        if let Some(value) = value {
            command.env(key, value);
        }
    }
    if let Some(cwd) = original.get_current_dir() {
        command.current_dir(cwd);
    }
    configure_stdio(&mut command, group, terminal)?;
    let controlling = terminal.is_some();
    os::configure_launcher(&mut command, controlling, fds);
    let deadline = Instant::now() + super::GRANT_TIMEOUT;
    let child = command.spawn().map_err(|e| {
        Error::spawn_failed("Cannot start the trusted provider launcher.").with_source(e)
    })?;
    os::register_child(child.id());
    drop(child_gate);
    Ok(PreparedSpawn {
        child,
        parent,
        deadline,
    })
}
pub(super) fn complete_launch(
    mut child: Child,
    mut parent: UnixStream,
    grant_deadline: Instant,
    authorize: impl FnOnce(u32) -> Result<()>,
) -> Result<Child> {
    // The shim is running and spawn's exec handshake has finished. Native code
    // cannot run until the caller has fsynced this PID/birth authorization.
    let authorization = authorize(child.id()).and_then(|()| {
        if Instant::now() >= grant_deadline {
            Err(Error::timeout(
                "Provider launch authorization exceeded its gate deadline.",
            ))
        } else {
            Ok(())
        }
    });
    if let Err(error) = authorization {
        // This gate does not own a credential receipt: authorization failure
        // kills the trusted shim and classifies errors from direct-child evidence.
        drop(parent);
        if cleanup_owned(&mut child) {
            return Err(
                Error::spawn_failed("Native execution was not authorized.").with_source(error)
            );
        }
        return Err(
            Error::provider_io("Trusted launch status could not be verified.").with_source(error),
        );
    }
    if let Err(error) = parent.write_all(b"G") {
        if cleanup_owned(&mut child) {
            // write_all of one byte can fail only before that byte is written.
            return Err(
                Error::spawn_failed("Native execution grant was not sent.").with_source(error)
            );
        }
        return Err(
            Error::provider_io("Trusted launch status could not be verified.").with_source(error),
        );
    }
    // The shim closes its endpoint on successful exec; only a complete tagged
    // errno proves exec failed. EOF alone is not proof that native code never ran.
    let deadline = Instant::now() + super::STATUS_TIMEOUT;
    let mut status = Vec::new();
    let result = read_status(&mut parent, deadline, &mut status);
    if status.len() == STATUS_BYTES && &status[..TAG_BYTES] == STATUS_TAG {
        #[expect(clippy::expect_used, reason = "Validated four-byte errno record.")]
        let errno = i32::from_ne_bytes(status[TAG_BYTES..].try_into().expect("four errno bytes"));
        let error = io::Error::from_raw_os_error(errno);
        return Err(if cleanup_owned(&mut child) {
            Error::spawn_failed(super::NATIVE_EXEC_FAILED).with_source(error)
        } else {
            Error::provider_io("Trusted launch status could not be verified.").with_source(error)
        });
    }
    if result.is_err() || !status.is_empty() {
        cleanup_owned(&mut child);
        let error = Error::provider_io("Trusted launch status could not be verified.");
        return Err(match result {
            Err(cause) => error.with_source(cause),
            Ok(()) => error,
        });
    }
    Ok(child)
}

fn cleanup_owned(child: &mut Child) -> bool {
    let attempt = hive_setup_support::child::kill_owned(child, |child| {
        os::signal_group(child, os::Signal::KILL)
    });
    for (phase, error) in attempt.errors() {
        tracing::warn!(name: "launch.cleanup_failed", pid = child.id(), phase = phase,
            error = error as &(dyn std::error::Error + 'static),
            decision = "preserve_launch_error");
    }
    if attempt.ownership_error.is_some() {
        return false;
    }
    let deadline = Instant::now() + hive_setup_support::child::FORCED_REAP_GRACE;
    match cleanup(
        hive_setup_support::child::reap_until(child, deadline),
        "reap_shim",
        child.id(),
    ) {
        Some(Some(_)) => true,
        Some(None) => {
            tracing::warn!(name: "launch.cleanup_unconfirmed", pid = child.id(),
                code = "reap_timeout", decision = "retain_evidence");
            false
        }
        None => false,
    }
}

fn cleanup<T>(result: io::Result<T>, phase: &'static str, pid: u32) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::warn!(name: "launch.cleanup_failed",
                 pid = pid,
                 phase = phase,
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "preserve_launch_error");
            None
        }
    }
}

fn configure_stdio(command: &mut Command, group: bool, terminal: Option<&fs::File>) -> Result<()> {
    if let Some(terminal) = terminal {
        command
            .stdin(terminal.try_clone().map_err(Error::state_io_from)?)
            .stdout(terminal.try_clone().map_err(Error::state_io_from)?)
            .stderr(terminal.try_clone().map_err(Error::state_io_from)?);
    } else if group {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    } else {
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
    }
    Ok(())
}

fn read_status(parent: &mut UnixStream, deadline: Instant, status: &mut Vec<u8>) -> io::Result<()> {
    // Gate waits preserve the last I/O error and poll descriptor readiness;
    // they do not have the fixed sleep/boolean result of process::wait_until.
    loop {
        let mut bytes = [0; STATUS_READ_BYTES];
        match parent.read(&mut bytes) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                status.extend_from_slice(&bytes[..n]);
                if status.len() >= STATUS_BYTES {
                    return Ok(());
                }
            }
            Err(e)
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted)
                    && Instant::now() < deadline =>
            {
                os::poll_readable(
                    [Some(parent.as_fd()), None, None],
                    deadline.saturating_duration_since(Instant::now()),
                )?;
            }
            Err(e) => return Err(e),
        }
    }
}
