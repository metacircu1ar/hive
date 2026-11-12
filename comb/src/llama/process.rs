//! The owned server child: loopback spawn, readiness, a bounded private stderr
//! tail, and stop-and-reap. Nothing here restarts a server.
use super::{HOST, Pin, STDERR_TAIL_BYTES, STOP_GRACE, probe};
use crate::{Error, ErrorCode, Result};
use hive_setup_support::child::{FORCED_REAP_GRACE, has_exited, reap_until};
use std::{
    collections::VecDeque,
    ffi::OsString,
    io::{self, Read},
    net::{SocketAddr, TcpStream},
    os::unix::process::ExitStatusExt,
    path::PathBuf,
    process::{Child, ChildStderr, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

mod health;
use health::{Health, client as health_client, health};

const POLL: Duration = Duration::from_millis(250);
const CLEANUP_POLL: Duration = Duration::from_millis(10);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(1);
/// After an exit, wait this long for the stderr reader to collect the last lines.
const DRAIN_WAIT: Duration = Duration::from_secs(1);
/// The monitor wakes at least this often, so stopping it is bounded.
const MONITOR_JOIN: Duration = Duration::from_secs(5);

/// A validated server that is not running yet: program, argv and loopback port.
#[derive(Clone, Debug)]
pub struct Launch {
    pub(crate) program: PathBuf,
    pub(crate) arguments: Vec<OsString>,
    pub(crate) port: u16,
    pub(crate) pin: Pin,
}

/// The private stderr tail. It is attached as a failure cause, never logged.
#[derive(Debug)]
pub(crate) struct ServerOutput(String);
impl std::fmt::Display for ServerOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "llama-server output tail:\n{}", self.0)
    }
}
impl std::error::Error for ServerOutput {}

enum State {
    Live(Child),
    Exited(ExitStatus),
    Stopped,
    /// Reaping could not be confirmed; the PID is never signalled again.
    Lost(Error),
}
impl State {
    /// The child is gone, so its stderr reaches EOF unless an inheritor holds it.
    fn exited(&self) -> bool {
        matches!(self, Self::Exited(_) | Self::Stopped)
    }
}

struct Shared {
    state: Mutex<State>,
    tail: Mutex<VecDeque<u8>>,
    /// The stderr reader reached EOF or stopped.
    drained: AtomicBool,
    stopping: AtomicBool,
}

/// What a serving Comb knows about its managed server: nothing, until the monitor
/// records the one failure. Clones share that record. Nothing clears it, because
/// nothing restarts the server; restarting Comb starts a new one.
#[derive(Clone, Debug, Default)]
pub struct Availability(Arc<Mutex<Option<Error>>>);
impl Availability {
    /// After a failure, embedding work gets this error instead of a refused
    /// connection. Text search never depends on it.
    pub fn check(&self) -> Result<()> {
        match &*lock(&self.0) {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    /// Keep the first failure, as `embeddings_unavailable` with the recovery steps;
    /// the specific failure and its private output tail stay the cause.
    pub(crate) fn record(&self, failure: Error) {
        let error = if failure.code == ErrorCode::EmbeddingsUnavailable {
            failure // A server that stopped after readiness already says so.
        } else {
            Error::llama_server_stopped().caused_by(failure)
        };
        let mut recorded = lock(&self.0);
        if recorded.is_none() {
            *recorded = Some(error);
        }
    }
}

/// A started server. Dropping it stops and reaps the child.
pub struct Running {
    shared: Arc<Shared>,
    port: u16,
    started: Instant,
    client: reqwest::blocking::Client,
    monitor: Option<JoinHandle<()>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // The guarded values stay consistent after a panic elsewhere; keep stopping.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Refuse a port another process already serves: health checks must reach ours.
pub(crate) fn require_idle(port: u16) -> Result<()> {
    match TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), HEALTH_TIMEOUT) {
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => Ok(()),
        Ok(_) => Err(Error::llama_server_port_in_use(port)),
        Err(error) => Err(Error::llama_server_port_in_use(port).caused_by(error)),
    }
}

impl Launch {
    /// Check the binary's version (an untested one is only a warning), then start
    /// it on its idle loopback port. The child stays in Comb's process group, so a
    /// terminal or service manager that signals Comb's group reaches it too.
    pub fn spawn(&self, cancelled: &dyn Fn() -> bool) -> Result<Running> {
        if !probe::version(&self.program, &self.pin, cancelled)? {
            tracing::warn!(name: "comb.llama_untested_version", decision = "warn_only");
        }
        require_idle(self.port)?;
        if cancelled() {
            return Err(io::Error::from(io::ErrorKind::Interrupted).into());
        }
        let client = health_client()?;
        let mut command = Command::new(&self.program);
        command
            .args(&self.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        // The argv is authoritative; ambient server defaults must not change it.
        for (name, _) in std::env::vars_os() {
            if name.as_encoded_bytes().starts_with(b"LLAMA_ARG_") {
                command.env_remove(name);
            }
        }
        orphan_guard(&mut command);
        let mut child = command.spawn()?;
        let stderr = child.stderr.take();
        tracing::info!(name: "comb.llama_started", pid = child.id());
        let running = Running {
            shared: Arc::new(Shared {
                state: Mutex::new(State::Live(child)),
                tail: Mutex::new(VecDeque::new()),
                drained: AtomicBool::new(stderr.is_none()),
                stopping: AtomicBool::new(false),
            }),
            port: self.port,
            started: Instant::now(),
            client,
            monitor: None,
        };
        if let Some(stderr) = stderr {
            let shared = running.shared.clone();
            // Detached: it ends at EOF, after the child and any inheritor exit.
            drop(
                std::thread::Builder::new()
                    .name("comb-llama-stderr".into())
                    .spawn(move || drain(stderr, &shared))?,
            );
        }
        Ok(running)
    }
}

/// Linux delivers SIGKILL to the child when Comb's spawning thread dies, so a
/// crashed Comb never leaves a server holding 8 GB. Both spawn sites run on the
/// process's main thread.
#[cfg(target_os = "linux")]
fn orphan_guard(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: getpid only observes this process's immutable native identity.
    let parent = unsafe { libc::getpid() };
    // SAFETY: these calls are async-signal-safe; the closure only sets the death
    // signal and checks whether the parent died before that signal was installed.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                return Err(io::Error::from_raw_os_error(libc::ECHILD));
            }
            Ok(())
        });
    }
}
#[cfg(not(target_os = "linux"))]
fn orphan_guard(_: &mut Command) {}

fn drain(stream: ChildStderr, shared: &Shared) {
    read_tail(stream, shared);
    shared.drained.store(true, Ordering::Release);
}

fn read_tail(mut stream: ChildStderr, shared: &Shared) {
    let mut buffer = [0; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return,
            Ok(count) => {
                let mut tail = lock(&shared.tail);
                tail.extend(&buffer[..count]);
                let excess = tail.len().saturating_sub(STDERR_TAIL_BYTES);
                tail.drain(..excess);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
            Err(error) => {
                tracing::warn!(name: "comb.llama_output_failed",
                    error = &error as &(dyn std::error::Error + 'static),
                    decision = "stop_reading");
                return;
            }
        }
    }
}

fn exit_status(shared: &Shared) -> Result<Option<ExitStatus>> {
    let mut state = lock(&shared.state);
    match &mut *state {
        State::Live(child) => {
            // Observation keeps the PID owned; only an exited child is reaped.
            match has_exited(child).and_then(|exited| {
                if exited {
                    child.wait().map(Some)
                } else {
                    Ok(None)
                }
            }) {
                Ok(Some(status)) => {
                    *state = State::Exited(status);
                    Ok(Some(status))
                }
                Ok(None) => Ok(None),
                Err(cause) => {
                    let error = Error::from(cause);
                    *state = State::Lost(error.clone());
                    Err(error)
                }
            }
        }
        State::Exited(status) => Ok(Some(*status)),
        State::Stopped => Ok(None),
        State::Lost(error) => Err(error.clone()),
    }
}

fn exited_error(shared: &Shared, status: ExitStatus, ready: bool) -> Error {
    tracing::error!(name: "comb.llama_exited",
        exit_code = status.code().unwrap_or(-1),
        signal = status.signal().unwrap_or(0),
        decision = "no_restart");
    let error = if ready {
        Error::llama_server_stopped()
    } else {
        Error::llama_server_exited(status.code(), status.signal())
    };
    error.caused_by(output(shared))
}

fn output(shared: &Shared) -> ServerOutput {
    let until = Instant::now() + DRAIN_WAIT;
    while lock(&shared.state).exited() && !shared.drained.load(Ordering::Acquire) {
        if Instant::now() >= until {
            break;
        }
        std::thread::sleep(CLEANUP_POLL);
    }
    let tail = lock(&shared.tail);
    ServerOutput(String::from_utf8_lossy(&tail.iter().copied().collect::<Vec<_>>()).into_owned())
}

impl Running {
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The live child's PID, while it is still owned and unreaped.
    #[cfg(test)]
    pub(super) fn pid(&self) -> Option<u32> {
        match &*lock(&self.shared.state) {
            State::Live(child) => Some(child.id()),
            State::Exited(_) | State::Stopped | State::Lost(_) => None,
        }
    }

    /// Wait until `/health` reports ok. Cancellation, the deadline and an early
    /// exit end the wait; the caller still owns, and must stop, the child.
    pub fn wait_ready(&self, deadline: Instant, cancelled: &dyn Fn() -> bool) -> Result<()> {
        loop {
            if cancelled() {
                return Err(io::Error::from(io::ErrorKind::Interrupted).into());
            }
            if let Some(status) = exit_status(&self.shared)? {
                return Err(exited_error(&self.shared, status, false));
            }
            if health(&self.client, self.port)? == Health::Ready {
                let elapsed = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
                tracing::info!(name: "comb.llama_ready", elapsed_ms = elapsed);
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::llama_server_not_ready().caused_by(output(&self.shared)));
            }
            std::thread::sleep(POLL);
        }
    }

    /// Watch the server in the background for a serving Comb: readiness within
    /// `deadline`, then any exit. A failure is recorded once in `availability`
    /// and the server is stopped, never restarted.
    pub fn monitor(mut self, deadline: Duration, availability: Availability) -> Result<Self> {
        let shared = self.shared.clone();
        let client = self.client.clone();
        let (port, deadline) = (self.port, self.started + deadline);
        self.monitor = Some(
            std::thread::Builder::new()
                .name("comb-llama-monitor".into())
                .spawn(move || watch(&shared, &availability, &client, port, deadline))?,
        );
        Ok(self)
    }

    /// SIGTERM, a bounded grace, then SIGKILL and a bounded reap.
    pub fn stop(mut self) -> Result<()> {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> Result<()> {
        self.shared.stopping.store(true, Ordering::Release);
        if let Some(monitor) = self.monitor.take() {
            let until = Instant::now() + MONITOR_JOIN;
            while !monitor.is_finished() && Instant::now() < until {
                std::thread::sleep(CLEANUP_POLL);
            }
            if monitor.is_finished() && monitor.join().is_err() {
                tracing::error!(name: "comb.llama_monitor_failed", decision = "continue_stop");
            }
        }
        // A monitor that is stopping a failed server holds the state until it is
        // reaped; an exited, stopped or lost child is never signalled here.
        retire(&self.shared)
    }
}

/// Stop and reap a live child. Uncertain cleanup stays a failure on later calls.
fn retire(shared: &Shared) -> Result<()> {
    let mut state = lock(&shared.state);
    let child = match &mut *state {
        State::Live(child) => child,
        State::Lost(error) => return Err(error.clone()),
        State::Exited(_) | State::Stopped => return Ok(()),
    };
    match terminate(child) {
        Ok(status) => {
            tracing::info!(name: "comb.llama_stopped",
                exit_code = status.code().unwrap_or(-1),
                signal = status.signal().unwrap_or(0));
            *state = State::Stopped;
            Ok(())
        }
        Err(error) => {
            *state = State::Lost(error.clone());
            Err(error)
        }
    }
}

fn terminate(child: &mut Child) -> Result<ExitStatus> {
    let pid = rustix::process::Pid::from_child(child);
    if !has_exited(child)? {
        rustix::process::kill_process(pid, rustix::process::Signal::TERM)
            .map_err(io::Error::from)?;
    }
    if let Some(status) = reap_until(child, Instant::now() + STOP_GRACE)? {
        return Ok(status);
    }
    // Still unreaped, so the PID is still ours; a zombie ignores the signal.
    rustix::process::kill_process(pid, rustix::process::Signal::KILL).map_err(io::Error::from)?;
    reap_until(child, Instant::now() + FORCED_REAP_GRACE)?
        .ok_or_else(Error::llama_server_stop_uncertain)
}

fn watch(
    shared: &Shared,
    availability: &Availability,
    client: &reqwest::blocking::Client,
    port: u16,
    deadline: Instant,
) {
    let mut ready = false;
    while !shared.stopping.load(Ordering::Acquire) {
        let failure = match exit_status(shared) {
            Ok(Some(status)) => Some(exited_error(shared, status, ready)),
            Ok(None) if ready => None,
            Ok(None) => match health(client, port) {
                Ok(Health::Ready) => {
                    ready = true;
                    tracing::info!(name: "comb.llama_ready", success = true);
                    None
                }
                Ok(Health::Loading) if Instant::now() >= deadline => {
                    Some(Error::llama_server_not_ready().caused_by(output(shared)))
                }
                Ok(Health::Loading) => None,
                Err(error) => Some(error),
            },
            Err(error) => Some(error),
        };
        if let Some(error) = failure {
            // Read before any stop request: a stopping Comb is not a failure.
            if !shared.stopping.load(Ordering::Acquire) {
                tracing::error!(name: "comb.llama_failed",
                    code = error.code.as_str(),
                    decision = "text_search_only");
                availability.record(error);
                // A server that never became ready, or answers wrongly, would keep
                // its memory while Comb serves text search. An exited one is reaped.
                if let Err(secondary) = retire(shared) {
                    tracing::error!(name: "comb.llama_stop_failed",
                        code = secondary.code.as_str(),
                        error = &secondary as &(dyn std::error::Error + 'static),
                        decision = "retain_process");
                }
            }
            return;
        }
        std::thread::sleep(POLL);
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        // An explicit stop already reported its outcome. Never turn its failure
        // into a retry or emit a duplicate cleanup failure from Drop.
        if self.shared.stopping.load(Ordering::Acquire) {
            return;
        }
        if let Err(error) = self.stop_inner() {
            tracing::error!(name: "comb.llama_stop_failed",
                code = error.code.as_str(),
                error = &error as &(dyn std::error::Error + 'static),
                decision = "retain_process");
        }
    }
}

#[cfg(test)]
mod tests;
