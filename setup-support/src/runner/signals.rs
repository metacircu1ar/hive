//! One serialized native signal owner for synchronous CLI child supervision.
use crate::Result;
use std::{
    io,
    os::unix::process::CommandExt,
    process::Command,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};
static OWNER: Mutex<()> = Mutex::new(());
static CANCELLED: AtomicBool = AtomicBool::new(false);
const SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];
extern "C" fn cancel(_: libc::c_int) {
    // Lock-free signal-safe state; the serialized owner remains alive throughout.
    CANCELLED.store(true, Ordering::Relaxed);
}
/// Owns process dispositions until every child supervised by this scope is retired.
/// Raw capture helpers never acquire another scope; do not nest scopes.
pub struct Signals {
    _owner: MutexGuard<'static, ()>,
    previous: Vec<(libc::c_int, libc::sigaction)>,
    flag: &'static AtomicBool,
}
impl Signals {
    pub fn new() -> Result<Self> {
        let owner = OWNER.lock().map_err(|poison| {
            // PoisonError owns the guard; never retain that lock inside an error.
            drop(poison);
            io::Error::other("Setup cancellation owner is poisoned.")
        })?;
        CANCELLED.store(false, Ordering::Relaxed);
        let mut state = Self {
            _owner: owner,
            previous: Vec::with_capacity(SIGNALS.len() + 1),
            flag: &CANCELLED,
        };
        // SAFETY: initialize every member before calling sigaction. Only this
        // serialized native scope owns these process dispositions.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = libc::SIG_DFL;
        // SAFETY: the action owns a live local signal mask.
        if unsafe { libc::sigemptyset(&raw mut action.sa_mask) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // Disable inherited SIG_IGN and SA_NOCLDWAIT before any owned spawn.
        // The unreaped leader pins its PID/PGID through final group signalling.
        // SAFETY: fully initialized C output storage for sigaction.
        let mut previous = unsafe { std::mem::zeroed() };
        // SAFETY: initialized C records; sa_flags=0 also clears auto-reap.
        if unsafe { libc::sigaction(libc::SIGCHLD, &raw const action, &raw mut previous) } != 0 {
            return Err(io::Error::last_os_error());
        }
        state.previous.push((libc::SIGCHLD, previous));
        action.sa_sigaction = cancel as *const () as usize;
        for signal in SIGNALS {
            // SAFETY: previous is fully initialized C output storage; both
            // action pointers stay valid for the complete call.
            let mut previous = unsafe { std::mem::zeroed() };
            // SAFETY: initialized live C input/output structures for this signal.
            if unsafe { libc::sigaction(signal, &raw const action, &raw mut previous) } != 0 {
                return Err(io::Error::last_os_error());
            }
            state.previous.push((signal, previous));
        }
        Ok(state)
    }
    pub fn cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
    pub fn configure_child(&self, command: &mut Command) {
        let previous = self.previous.clone();
        // SAFETY: restore the pre-supervision dispositions before foreground
        // handoff, so Ctrl-C cannot run a parent-only handler in the forked child.
        // The closure uses only async-signal-safe calls and immutable C records.
        unsafe {
            command.pre_exec(move || {
                for (signal, action) in &previous {
                    // Child owners also need default SIGCHLD; do not propagate
                    // an inherited auto-reap policy into setup or its helpers.
                    if *signal != libc::SIGCHLD
                        && libc::sigaction(*signal, action, std::ptr::null_mut()) != 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
    }
    pub fn finish<T, E: From<io::Error>>(
        mut self,
        result: std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        let cleanup = self.restore();
        let result = result.and_then(|value| {
            if self.cancelled() {
                Err(io::Error::from(io::ErrorKind::Interrupted).into())
            } else {
                Ok(value)
            }
        });
        match result {
            Ok(value) => {
                cleanup?;
                Ok(value)
            }
            Err(primary) => {
                if let Err(error) = cleanup {
                    tracing::error!(name: "setup.signal_restore_failed",
                        error = &error as &(dyn std::error::Error + 'static),
                        decision = "preserve_primary_failure");
                }
                Err(primary)
            }
        }
    }
    pub fn restore(&mut self) -> Result<()> {
        let mut errors: [Option<io::Error>; SIGNALS.len() + 1] = std::array::from_fn(|_| None);
        for ((signal, previous), error) in self.previous.drain(..).rev().zip(errors.iter_mut()) {
            // SAFETY: each saved action came from the matching successful call
            // above, and remains live while its exact disposition is restored.
            if unsafe { libc::sigaction(signal, &raw const previous, std::ptr::null_mut()) } != 0 {
                *error = Some(io::Error::last_os_error());
            }
        }
        let mut errors = errors.into_iter().flatten();
        let Some(first) = errors.next() else {
            return Ok(());
        };
        let rest: [Option<io::Error>; SIGNALS.len()] = std::array::from_fn(|_| errors.next());
        if rest.iter().all(Option::is_none) {
            Err(first)
        } else {
            Err(io::Error::new(first.kind(), RestoreErrors { first, rest }))
        }
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            tracing::error!(name: "setup.signal_restore_failed",
                error = &error as &(dyn std::error::Error + 'static),
                decision = "preserve_primary_failure");
        }
    }
}

/// Bounded native causes from one unsuccessful restoration, in attempted order.
#[derive(Debug)]
pub struct RestoreErrors {
    first: io::Error,
    rest: [Option<io::Error>; SIGNALS.len()],
}
impl RestoreErrors {
    pub fn errors(&self) -> impl Iterator<Item = &io::Error> {
        std::iter::once(&self.first).chain(self.rest.iter().flatten())
    }
}
impl std::fmt::Display for RestoreErrors {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str("Cannot restore native signal dispositions.")
    }
}
impl std::error::Error for RestoreErrors {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.first)
    }
}
