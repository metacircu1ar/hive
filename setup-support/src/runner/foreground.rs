//! Temporarily lend the original terminal to a private child process group.
use crate::Result;
use std::{
    fs::File,
    io,
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::Command,
};

pub(super) struct Foreground<'a> {
    terminal: Option<&'a File>,
    original: libc::pid_t,
    modes: Option<libc::termios>,
}
impl<'a> Foreground<'a> {
    pub(super) fn new(terminal: Option<&'a File>) -> Result<Self> {
        let original = match terminal {
            Some(file) => {
                // SAFETY: the descriptor is a live borrowed terminal.
                let group = unsafe { libc::tcgetpgrp(file.as_raw_fd()) };
                if group < 0 {
                    return Err(io::Error::last_os_error());
                }
                group
            }
            None => 0,
        };
        let modes = terminal
            .map(|file| {
                // SAFETY: termios is initialized C output storage for a live TTY.
                let mut modes = unsafe { std::mem::zeroed() };
                // SAFETY: the descriptor and output storage remain valid.
                if unsafe { libc::tcgetattr(file.as_raw_fd(), &raw mut modes) } != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(modes)
            })
            .transpose()?;
        Ok(Self {
            terminal,
            original,
            modes,
        })
    }
    pub(super) fn configure(&self, command: &mut Command) {
        if self.terminal.is_none() {
            return;
        }
        // SAFETY: only async-signal-safe calls and initialized stack storage are
        // used after fork. stdin is the terminal clone configured by the caller.
        unsafe {
            command.pre_exec(|| set_foreground(libc::STDIN_FILENO, libc::getpgrp()));
        }
    }
    pub(super) fn finish<T>(mut self, result: Result<T>) -> Result<T> {
        let cleanup = self.restore();
        match (result, cleanup) {
            (Ok(value), Ok(())) => Ok(value),
            (Ok(_), Err(error)) => Err(error),
            (Err(primary), Err(error)) => {
                tracing::error!(name: "setup.terminal_restore_failed",
                    error = &error as &(dyn std::error::Error + 'static),
                    decision = "preserve_primary_failure");
                Err(primary)
            }
            (Err(primary), Ok(())) => Err(primary),
        }
    }
    fn restore(&mut self) -> Result<()> {
        if let Some(file) = self.terminal.take() {
            let foreground = set_foreground(file.as_raw_fd(), self.original);
            let modes = self.modes.take().map_or(Ok(()), |modes| {
                blocked(|| {
                    // SAFETY: the saved modes came from this borrowed TTY and
                    // remain valid even after a child was forcibly killed.
                    if unsafe { libc::tcsetattr(file.as_raw_fd(), libc::TCSANOW, &raw const modes) }
                        == 0
                    {
                        Ok(())
                    } else {
                        Err(io::Error::last_os_error())
                    }
                })
            });
            match (foreground, modes) {
                (Err(primary), Err(error)) => {
                    tracing::error!(name: "setup.terminal_restore_failed", phase = "modes",
                        error = &error as &(dyn std::error::Error + 'static),
                        decision = "preserve_primary_failure");
                    return Err(primary);
                }
                (Err(error), _) | (_, Err(error)) => return Err(error),
                (Ok(()), Ok(())) => {}
            }
        }
        Ok(())
    }
}
impl Drop for Foreground<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            tracing::error!(name: "setup.terminal_restore_failed",
                error = &error as &(dyn std::error::Error + 'static),
                decision = "preserve_primary_failure");
        }
    }
}

fn set_foreground(fd: libc::c_int, group: libc::pid_t) -> Result<()> {
    blocked(|| {
        // SAFETY: the descriptor is a borrowed TTY and the group is owned.
        if unsafe { libc::tcsetpgrp(fd, group) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    })
}
fn blocked(operation: impl FnOnce() -> Result<()>) -> Result<()> {
    // Block SIGTTOU only on this thread: changing a background group's terminal
    // must not stop it, and changing a process-wide disposition would race logs.
    // SAFETY: C masks are initialized before use; all pointers remain live.
    unsafe {
        let mut mask = std::mem::zeroed();
        let mut previous = std::mem::zeroed();
        if libc::sigemptyset(&raw mut mask) != 0
            || libc::sigaddset(&raw mut mask, libc::SIGTTOU) != 0
        {
            return Err(io::Error::last_os_error());
        }
        let code = libc::pthread_sigmask(libc::SIG_BLOCK, &raw const mask, &raw mut previous);
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code));
        }
        let result = operation();
        let code =
            libc::pthread_sigmask(libc::SIG_SETMASK, &raw const previous, std::ptr::null_mut());
        result.and_then(|()| {
            if code == 0 {
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(code))
            }
        })
    }
}
