//! Sanitize descriptors only inside the single-threaded, already-executed launcher.
use std::{io, os::fd::RawFd};

pub(crate) fn mark_unlisted_cloexec(allowed: &[RawFd]) -> io::Result<()> {
    visit(|fd| {
        if fd > libc::STDERR_FILENO && !allowed.contains(&fd) {
            super::fd::close_retained_on_exec(fd)?;
        }
        Ok(())
    })
}

#[cfg(target_os = "linux")]
fn visit(mut apply: impl FnMut(RawFd) -> io::Result<()>) -> io::Result<()> {
    // Retain the directory iterator until every observed descriptor is flagged.
    // Closing it first would leave a stale FD in the snapshot.
    let entries = std::fs::read_dir("/proc/self/fd")?;
    for entry in entries {
        let fd = entry?
            .file_name()
            .to_str()
            .ok_or_else(super::invalid)?
            .parse()
            .map_err(|_| super::invalid())?;
        apply(fd)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn visit(mut apply: impl FnMut(RawFd) -> io::Result<()>) -> io::Result<()> {
    const MAX_FDS: usize = 65_536;
    let mut fds = vec![
        libc::proc_fdinfo {
            proc_fd: 0,
            proc_fdtype: 0
        };
        64
    ];
    loop {
        let bytes =
            i32::try_from(std::mem::size_of_val(fds.as_slice())).map_err(|_| super::invalid())?;
        // SAFETY: proc_pidinfo writes at most bytes into the initialized C-record buffer.
        let count = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                fds.as_mut_ptr().cast(),
                bytes,
            )
        };
        if count <= 0 {
            let error = io::Error::last_os_error();
            return Err(if error.raw_os_error() == Some(0) {
                super::invalid()
            } else {
                error
            });
        }
        let count = usize::try_from(count).map_err(|_| super::invalid())?;
        if count > std::mem::size_of_val(fds.as_slice())
            || count % std::mem::size_of::<libc::proc_fdinfo>() != 0
        {
            return Err(super::invalid());
        }
        if count == std::mem::size_of_val(fds.as_slice()) {
            if fds.len() >= MAX_FDS {
                return Err(super::invalid());
            }
            fds.resize(
                fds.len() * 2,
                libc::proc_fdinfo {
                    proc_fd: 0,
                    proc_fdtype: 0,
                },
            );
            continue;
        }
        for record in &fds[..count / std::mem::size_of::<libc::proc_fdinfo>()] {
            apply(record.proc_fd)?;
        }
        return Ok(());
    }
}
