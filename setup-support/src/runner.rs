//! Bounded manager subprocesses. The caller supplies its own fork-safe spawn.
mod cancellation;
mod foreground;
mod reaping;
mod signals;
pub use reaping::Reaping;
pub use signals::{RestoreErrors, Signals};
#[cfg(target_os = "macos")]
mod group;
use crate::{Result, invalid};
use std::{
    io::Read,
    os::unix::process::CommandExt,
    process::{Child, ChildStderr, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};
pub type Spawn = fn(&mut Command) -> Result<Child>;
#[derive(Debug)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}
impl Output {
    fn decode(status: std::process::ExitStatus, out: Vec<u8>, err: Vec<u8>) -> Result<Self> {
        Ok(Self {
            code: status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&err).into_owned(),
            stdout: String::from_utf8(out).map_err(|cause| {
                crate::context(
                    "Manager returned non-UTF-8 output",
                    std::io::Error::new(std::io::ErrorKind::InvalidData, cause),
                )
            })?,
        })
    }
}

pub trait Runner {
    fn run(&self, program: &str, args: &[String]) -> Result<Output>;
}
pub struct SystemRunner(pub Spawn);
impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        let signals = Signals::new()?;
        let mut command = Command::new(program);
        command.args(args);
        signals.configure_child(&mut command);
        let result = capture_configured(self.0, &mut command, Some(RUN_TIMEOUT), &|| {
            signals.cancelled()
        });
        signals.finish(result)
    }
}
#[derive(PartialEq, Eq)]
enum Ownership {
    Owned,
    Reaped,
    Lost,
}
struct Guard(Child, Ownership);
impl Guard {
    fn exited(&mut self) -> Result<bool> {
        crate::child::has_exited(&self.0).inspect_err(|_| self.1 = Ownership::Lost)
    }
    fn wait_ready(&mut self) -> Result<std::process::ExitStatus> {
        let status = self.0.wait();
        self.1 = if status.is_ok() {
            Ownership::Reaped
        } else {
            Ownership::Lost
        };
        status
    }
    fn kill_group(&self) -> Result<()> {
        kill_group(&self.0)
    }
}
fn kill_group(child: &Child) -> Result<()> {
    // Private group; leader is retained unreaped until after signalling.
    match rustix::process::kill_process_group(
        rustix::process::Pid::from_child(child),
        rustix::process::Signal::KILL,
    ) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        #[cfg(target_os = "macos")]
        Err(rustix::io::Errno::PERM) if group::exited_alone(child) => Ok(()),
        Err(error) => Err(error.into()),
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if self.1 == Ownership::Owned {
            let attempt = crate::child::kill_owned(&mut self.0, |child| kill_group(child));
            for (phase, error) in attempt.errors() {
                tracing::error!(name: "setup.child_cleanup_failed",
                     phase = phase, error = error as &(dyn std::error::Error + 'static),
                     decision = "preserve_primary_failure");
            }
            if attempt.ownership_error.is_some() {
                return;
            }
            let cleanup = crate::child::reap_until(
                &mut self.0,
                Instant::now() + crate::child::FORCED_REAP_GRACE,
            );
            match cleanup {
                Ok(Some(_)) => self.1 = Ownership::Reaped,
                Ok(None) => tracing::error!(name: "setup.child_cleanup_failed", phase = "reap",
                    code = "reap_timeout", decision = "preserve_primary_failure"),
                Err(error) => tracing::error!(name: "setup.child_cleanup_failed", phase = "reap",
                    error = &error as &(dyn std::error::Error + 'static),
                    decision = "preserve_primary_failure"),
            }
        }
    }
}
fn nonblocking(fd: &impl std::os::fd::AsFd) -> Result<()> {
    #[cfg(test)]
    if FAIL_NONBLOCKING.with(std::cell::Cell::get) {
        return Err(std::io::Error::from_raw_os_error(libc::EIO));
    }
    let flags = rustix::fs::fcntl_getfl(fd)?;
    rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK)?;
    Ok(())
}
fn drain(input: &mut impl Read, data: &mut Vec<u8>) -> Result<()> {
    loop {
        let mut buf = [0; 4096];
        match input.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                if data.len() + n > 65536 {
                    return Err(invalid("Manager output exceeds 64 KiB."));
                }
                data.extend_from_slice(&buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => (),
            Err(e) => return Err(e),
        }
    }
}
pub fn capture(spawn: Spawn, program: &str, args: &[String], timeout: Duration) -> Result<Output> {
    capture_inner(spawn, program, args, Some(timeout), &|| false)
}
/// Long native operations own their transport deadlines. Show progress directly
/// on stderr and bound only the structured stdout, while retaining group cleanup.
pub fn progress(
    spawn: Spawn,
    program: &str,
    args: &[String],
    cancelled: &dyn Fn() -> bool,
) -> Result<Output> {
    capture_inner(spawn, program, args, None, cancelled)
}
fn capture_inner(
    spawn: Spawn,
    program: &str,
    args: &[String],
    timeout: Option<Duration>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Output> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    capture_configured(spawn, &mut cmd, timeout, cancelled)
}

/// Capture a caller-configured command with bounded output, time and group cleanup.
pub fn capture_command(
    spawn: Spawn,
    command: &mut Command,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<Output> {
    capture_configured(spawn, command, Some(timeout), cancelled)
}

/// Supervise a native CLI that must retire its own separately grouped children.
/// `None` preserves visible stderr progress while bounding structured stdout.
/// The caller supplies its signal scope; this function never creates a nested one.
pub fn owner_command(
    spawn: Spawn,
    command: &mut Command,
    timeout: Option<Duration>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Output> {
    if cancelled() {
        return Err(std::io::ErrorKind::Interrupted.into());
    }
    command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(if timeout.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        });
    let mut child = Guard(spawn(command)?, Ownership::Owned);
    let stdout = child.0.stdout.take();
    let stderr = child.0.stderr.take();
    finish_owner(child, stdout, stderr, timeout, cancelled)
}

fn capture_configured(
    spawn: Spawn,
    cmd: &mut Command,
    timeout: Option<Duration>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Output> {
    if cancelled() {
        return Err(std::io::ErrorKind::Interrupted.into());
    }
    cmd.process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(if timeout.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        });
    let mut child = Guard(spawn(cmd)?, Ownership::Owned);
    let mut stdout = child
        .0
        .stdout
        .take()
        .ok_or_else(|| invalid("Missing stdout pipe."))?;
    let mut stderr = child.0.stderr.take();
    nonblocking(&stdout)?;
    if let Some(stderr) = &stderr {
        nonblocking(stderr)?;
    }
    let mut out = Vec::new();
    let mut err = Vec::new();
    let deadline = timeout.map(|t| Instant::now() + t);
    loop {
        drain(&mut stdout, &mut out)?;
        if let Some(stderr) = &mut stderr {
            drain(stderr, &mut err)?;
        }
        if cancelled() {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        let exited = child.exited()?;
        if exited {
            child.kill_group()?;
            drain(&mut stdout, &mut out)?;
            if let Some(stderr) = &mut stderr {
                drain(stderr, &mut err)?;
            }
            let status = child.wait_ready()?;
            return Output::decode(status, out, err);
        }
        std::thread::sleep(CAPTURE_POLL);
    }
}

const RUN_TIMEOUT: std::time::Duration = Duration::from_secs(30);

const CAPTURE_POLL: std::time::Duration = Duration::from_millis(10);

/// Own a bounded process group while letting its questions use the controlling TTY.
/// The caller supplies the three streams; bootstrap stdin need never be consumed.
pub fn attached_command(
    command: &mut Command,
    terminal: Option<&std::fs::File>,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<Output> {
    if cancelled() {
        return Err(std::io::ErrorKind::Interrupted.into());
    }
    let foreground = foreground::Foreground::new(terminal)?;
    command.process_group(0);
    foreground.configure(command);
    if terminal.is_none() {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    }
    let mut child = Guard(command.spawn()?, Ownership::Owned);
    let stdout = child.0.stdout.take();
    let stderr = child.0.stderr.take();
    let result = finish_owner(child, stdout, stderr, Some(timeout), cancelled);
    foreground.finish(result)
}
fn finish_owner(
    mut child: Guard,
    mut stdout: Option<ChildStdout>,
    mut stderr: Option<ChildStderr>,
    timeout: Option<Duration>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Output> {
    let result = supervise(&mut child, &mut stdout, &mut stderr, timeout, cancelled);
    if result.is_err()
        && child.1 == Ownership::Owned
        && let Err(error) = cancellation::graceful(&mut child, &mut stdout, &mut stderr)
    {
        tracing::error!(name: "setup.child_cancel_failed", phase = "graceful",
            error = &error as &(dyn std::error::Error + 'static),
            decision = "preserve_primary_failure");
    }
    // Kill/reap independently, including failed observation and graceful cleanup.
    drop(child);
    result
}
fn supervise(
    child: &mut Guard,
    stdout: &mut Option<ChildStdout>,
    stderr: &mut Option<ChildStderr>,
    timeout: Option<Duration>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Output> {
    initialize_pipes(stdout, stderr)?;
    let mut out = Vec::with_capacity(4096);
    let mut err = Vec::with_capacity(4096);
    let deadline = timeout.map(|timeout| Instant::now() + timeout);
    loop {
        if let Some(stream) = stdout.as_mut() {
            drain(stream, &mut out)?;
        }
        if let Some(stream) = stderr.as_mut() {
            drain(stream, &mut err)?;
        }
        if cancelled() || crate::child::has_stopped(&child.0)? {
            break Err(std::io::ErrorKind::Interrupted.into());
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break Err(std::io::ErrorKind::TimedOut.into());
        }
        if child.exited()? {
            child.kill_group()?;
            if let Some(stream) = stdout.as_mut() {
                drain(stream, &mut out)?;
            }
            if let Some(stream) = stderr.as_mut() {
                drain(stream, &mut err)?;
            }
            break child
                .wait_ready()
                .and_then(|status| Output::decode(status, out, err));
        }
        std::thread::sleep(CAPTURE_POLL);
    }
}
fn initialize_pipes(
    stdout: &mut Option<ChildStdout>,
    stderr: &mut Option<ChildStderr>,
) -> Result<()> {
    let output = initialize_pipe(stdout);
    let error = initialize_pipe(stderr);
    match (output, error) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(primary), Err(error)) => {
            tracing::error!(name: "setup.capture_initialization_failed", phase = "stderr",
                error = &error as &(dyn std::error::Error + 'static),
                decision = "preserve_primary_failure");
            Err(primary)
        }
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}
fn initialize_pipe<T: std::os::fd::AsFd>(stream: &mut Option<T>) -> Result<()> {
    let result = stream.as_ref().map_or(Ok(()), nonblocking);
    if result.is_err() {
        *stream = None;
    }
    result
}
#[cfg(test)]
thread_local! { static FAIL_NONBLOCKING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
#[cfg(test)]
pub(crate) fn with_nonblocking_failure<T>(work: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            FAIL_NONBLOCKING.with(|flag| flag.set(self.0));
        }
    }
    let _reset = Reset(FAIL_NONBLOCKING.with(|flag| flag.replace(true)));
    work()
}
