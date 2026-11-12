//! Available memory against the named floor, checked before any model byte is read.
use super::MEMORY_FLOOR_BYTES;
use crate::{Error, Result};
#[cfg(any(target_os = "linux", test))]
use std::path::Path;
#[cfg(any(target_os = "linux", test))]
mod cgroup;

/// Refuse with the needed amount when `available` is below the floor.
pub fn admit(available: u64) -> Result<()> {
    if available < MEMORY_FLOOR_BYTES {
        return Err(Error::llama_memory_insufficient(
            available,
            MEMORY_FLOOR_BYTES,
        ));
    }
    Ok(())
}

/// Physical memory: unified memory has no meaningful available figure before a
/// Metal server allocates its buffers.
#[cfg(target_os = "macos")]
pub fn available() -> Result<u64> {
    let mut value: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    // SAFETY: hw.memsize is a 64-bit integer; both output pointers reference live
    // locals of the declared size, and no new value is written.
    let status = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            std::ptr::from_mut(&mut value).cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return Err(Error::llama_memory_unknown().caused_by(std::io::Error::last_os_error()));
    }
    if size != std::mem::size_of::<u64>() || value == 0 {
        return Err(Error::llama_memory_unknown());
    }
    Ok(value)
}

/// `MemAvailable`, bounded by every enforced cgroup limit on this process's path.
#[cfg(target_os = "linux")]
pub fn available() -> Result<u64> {
    linux(Path::new("/proc"))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn available() -> Result<u64> {
    Err(Error::llama_memory_unknown())
}

/// Pure parsing of a Linux `/proc` and cgroup tree; tests supply synthetic trees.
#[cfg(any(target_os = "linux", test))]
pub(super) fn linux(proc: &Path) -> Result<u64> {
    let meminfo = std::fs::read_to_string(proc.join("meminfo"))?;
    let available = meminfo
        .lines()
        .find_map(|line| {
            let mut fields = line.strip_prefix("MemAvailable:")?.split_whitespace();
            match (fields.next(), fields.next(), fields.next()) {
                (Some(kib), Some("kB"), None) => kib.parse::<u64>().ok()?.checked_mul(1024),
                _ => None,
            }
        })
        .ok_or_else(Error::llama_memory_unknown)?;
    cgroup::bound(proc, available)
}
