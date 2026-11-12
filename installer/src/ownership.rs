//! Distinguish trusted routes, private management and owned payload observations.
use crate::{Result, manifest};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};
#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
pub(crate) enum Directory {
    Manager,
    Inner,
}

pub(crate) fn routes(path: &Path) -> Result<()> {
    // Keep existing selection semantics for user roots (including /var on macOS).
    // Managed children are checked without resolving their leaf symlinks.
    let path = hive_setup_support::files::absolute(path)?;
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta)
                if meta.is_dir()
                    && hive_setup_support::directories::ancestor(
                        meta.uid(),
                        meta.mode(),
                        rustix::process::geteuid().as_raw(),
                    ) => {}
            Ok(_) => return Err("Installation route has an untrusted directory".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(crate) fn directory(path: &Path, class: Directory) -> Result<fs::Metadata> {
    let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::DIRECTORY;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits().try_into()?)
        .open(path)?;
    let meta = file.metadata()?;
    let user = rustix::process::geteuid().as_raw();
    let owned = match class {
        Directory::Manager => meta.uid() == user,
        Directory::Inner => hive_setup_support::directories::owned(meta.uid(), meta.mode(), user),
    };
    if !meta.is_dir() || !owned || (matches!(class, Directory::Manager) && meta.mode() & 0o077 != 0)
    {
        return Err(
            "Installation directory ownership or permissions are unsafe; retain it unchanged"
                .into(),
        );
    }
    named(path, &meta)?;
    Ok(meta)
}

pub(crate) fn regular(path: &Path) -> Result<File> {
    let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits().try_into()?)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err("Release input must be a regular file".into());
    }
    named(path, &meta)?;
    Ok(file)
}

pub(crate) fn owned(path: &Path) -> Result<File> {
    let file = regular(path)?;
    let meta = file.metadata()?;
    if meta.uid() != rustix::process::geteuid().as_raw() || meta.mode() & 0o022 != 0 {
        return Err(
            "Installation file ownership or permissions are unsafe; retain it unchanged".into(),
        );
    }
    Ok(file)
}

pub(crate) fn owner_record(path: &Path, expected: &[u8]) -> Result<()> {
    if record(path, manifest::byte_count(expected.len()))? != expected {
        return Err("Unrecognized installation owner".into());
    }
    Ok(())
}
pub(crate) fn record(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = owned(path)?;
    let meta = file.metadata()?;
    if meta.nlink() != 1 || meta.len() > limit {
        return Err("Installation record is oversized or has multiple links".into());
    }
    let bytes = manifest::read_from(&file, limit)?;
    named(path, &meta)?;
    Ok(bytes)
}

pub(crate) fn named(path: &Path, held: &fs::Metadata) -> Result<()> {
    let current = fs::symlink_metadata(path)?;
    if current.file_type().is_symlink()
        || (current.dev(), current.ino()) != (held.dev(), held.ino())
    {
        return Err("Installation filesystem identity changed; retain it unchanged".into());
    }
    Ok(())
}
