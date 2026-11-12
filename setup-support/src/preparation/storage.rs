use crate::{Result, invalid};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};
const LIMIT: u64 = 256 * 1024;

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn private_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Preparation needs a parent."))?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(parent)?;
    let meta = file.metadata()?;
    if !meta.is_dir()
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o077 != 0
    {
        return Err(invalid(
            "Setup preparation needs its own private directory.",
        ));
    }
    named(parent, &meta)
}
fn named(path: &Path, meta: &fs::Metadata) -> Result<()> {
    let actual = fs::symlink_metadata(path)?;
    if actual.file_type().is_symlink() || (actual.dev(), actual.ino()) != (meta.dev(), meta.ino()) {
        return Err(invalid(
            "Setup preparation identity changed; retain it unchanged.",
        ));
    }
    Ok(())
}
pub fn read_private(path: &Path) -> Result<Vec<u8>> {
    private_parent(path)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o077 != 0
        || meta.nlink() != 1
        || meta.len() > LIMIT
    {
        return Err(invalid("Setup preparation file is unsafe or oversized."));
    }
    let mut bytes = Vec::with_capacity(4096);
    (&file).take(LIMIT + 1).read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).map_err(io::Error::other)? > LIMIT {
        return Err(invalid("Setup preparation exceeds its size limit."));
    }
    if bytes.len() as u64 != meta.len() || !unchanged(&meta, &file.metadata()?) {
        return Err(invalid("Setup preparation changed while reading."));
    }
    named(path, &meta)?;
    Ok(bytes)
}
pub fn read<T: DeserializeOwned>(path: &Path, expected: Option<&str>) -> Result<T> {
    let bytes = read_private(path)?;
    if expected.is_some_and(|expected| digest(&bytes) != expected) {
        return Err(invalid("Prepared setup bytes changed; repeat preparation."));
    }
    hive_input::decode::from_slice(&bytes)
        .map_err(|cause| io::Error::new(io::ErrorKind::InvalidData, RecordError(cause)))
}
pub fn write(path: &Path, value: &impl Serialize) -> Result<String> {
    private_parent(path)?;
    let bytes = serde_json::to_vec(value)
        .map_err(|cause| io::Error::new(io::ErrorKind::InvalidData, RecordError(cause)))?;
    if u64::try_from(bytes.len()).map_err(io::Error::other)? > LIMIT {
        return Err(invalid("Setup preparation exceeds its size limit."));
    }
    atomic_fs::Publisher::new(crate::names::PUBLICATION_OPTIONS.with_mode(0o600))
        .create_new(path, &bytes)
        .map_err(io::Error::other)?;
    Ok(digest(&bytes))
}
pub fn executable_digest() -> Result<String> {
    let path = std::env::current_exe()?.canonicalize()?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    let before = file.metadata()?;
    if !before.is_file() || before.len() > 512 * 1024 * 1024 {
        return Err(invalid("Setup executable is not a bounded regular file."));
    }
    let mut hash = Sha256::new();
    let copied = io::copy(&mut (&file).take(512 * 1024 * 1024 + 1), &mut hash)?;
    let after = file.metadata()?;
    if copied != before.len() || !unchanged(&before, &after) {
        return Err(invalid(
            "Setup executable changed while checking its identity.",
        ));
    }
    named(&path, &before)?;
    Ok(format!("{:x}", hash.finalize()))
}
fn unchanged(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    (
        before.dev(),
        before.ino(),
        before.len(),
        before.mtime(),
        before.mtime_nsec(),
        before.ctime(),
        before.ctime_nsec(),
        before.uid(),
        before.mode(),
        before.nlink(),
    ) == (
        after.dev(),
        after.ino(),
        after.len(),
        after.mtime(),
        after.mtime_nsec(),
        after.ctime(),
        after.ctime_nsec(),
        after.uid(),
        after.mode(),
        after.nlink(),
    )
}

#[derive(Debug)]
struct RecordError(serde_json::Error);
impl std::fmt::Display for RecordError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Setup preparation record is malformed.")
    }
}
impl std::error::Error for RecordError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}
