//! Bounded safe reads, private path checks and page-path access.
use super::{Disk, Durability};
use crate::{Error, Result, page, values::Slug};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub(crate) fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
pub(crate) fn dir(path: &Path, create: bool, durability: &dyn Durability) -> Result<()> {
    directory(
        path,
        if create {
            Access::Durable
        } else {
            Access::Inspect
        },
        durability,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    Inspect,
    Durable,
    Ordered,
}
fn directory(path: &Path, access: Access, durability: &dyn Durability) -> Result<()> {
    if access != Access::Inspect
        && !exists(path)?
        && let Err(error) = fs::DirBuilder::new().mode(0o700).create(path)
        && error.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(error.into());
    }
    directory_metadata(path)?;
    if access != Access::Inspect {
        let parent = File::open(path.parent().ok_or_else(Error::parent_directory_missing)?)?;
        match access {
            Access::Durable => durability.sync_directory(&parent)?,
            Access::Ordered => durability.fsync(&parent)?,
            Access::Inspect => unreachable!("inspection has no directory barrier"),
        }
    }
    Ok(())
}
pub(super) fn directory_metadata(path: &Path) -> Result<fs::Metadata> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)
        .map_err(|cause| {
            if matches!(cause.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR)) {
                Error::unsafe_directory().caused_by(cause)
            } else {
                cause.into()
            }
        })?;
    #[cfg(test)]
    super::durable::checkpoint(
        path,
        hive_setup_support::checkpoints::Checkpoint::RecordOpened,
    )?;
    let meta = file.metadata()?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != rustix::process::geteuid().as_raw()
        || !hive_setup_support::directories::owned(
            meta.uid(),
            meta.mode(),
            rustix::process::geteuid().as_raw(),
        )
    {
        return Err(Error::unsafe_directory());
    }
    check_name(&file, path)?;
    let named = fs::symlink_metadata(path)?;
    if !named.is_dir() || (named.dev(), named.ino()) != (meta.dev(), meta.ino()) {
        return Err(Error::unsafe_directory());
    }
    Ok(meta)
}
impl Disk {
    pub fn page_path(&self, slug: &Slug, create: bool) -> Result<PathBuf> {
        self.locate_page(
            slug,
            if create {
                Access::Durable
            } else {
                Access::Inspect
            },
        )
    }
    // The durable intent's later full flush covers these ordered parent entries.
    pub(super) fn journal_page_path(&self, slug: &Slug) -> Result<PathBuf> {
        self.locate_page(slug, Access::Ordered)
    }
    fn locate_page(&self, slug: &Slug, access: Access) -> Result<PathBuf> {
        let mut path = self.root.clone();
        let device = self.root_device;
        let pieces: Vec<_> = slug.as_str().split('/').collect();
        for part in &pieces[..pieces.len() - 1] {
            path.push(part);
            match fs::symlink_metadata(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && access == Access::Inspect => {
                }
                _ => {
                    directory(&path, access, self.durability.as_ref())?;
                    same_device(&path, device)?;
                }
            }
        }
        path.push(format!("{}.md", pieces[pieces.len() - 1]));
        if exists(&path)? {
            same_device(&path, device)?;
        }
        Ok(path)
    }
    pub fn content(&self, slug: &Slug) -> Result<Option<String>> {
        let path = self.page_path(slug, false)?;
        match read(&path, page::PAGE_BYTES) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|cause| Error::page_not_utf8().caused_by(cause.utf8_error())),
            Err(error) if error.is_missing_file() => Ok(None),
            Err(error) => Err(error),
        }
    }
}

pub(super) fn same_device(path: &Path, expected: u64) -> Result<()> {
    if fs::symlink_metadata(path)?.dev() != expected {
        return Err(Error::cross_device_comb());
    }
    Ok(())
}

pub(super) fn create_root(path: &Path, durability: &dyn Durability) -> Result<()> {
    if exists(path)? {
        // Only the deepest existing ancestor can have been created without its
        // immediate parent barrier. Do not flush unrelated ancestors up to /.
        return match sync_root_parent(path, durability) {
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied
                        | std::io::ErrorKind::ReadOnlyFilesystem
                        | std::io::ErrorKind::Unsupported
                ) || error.raw_os_error() == Some(libc::ENOTSUP) =>
            {
                Ok(())
            }
            result => result.map_err(Error::from),
        };
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        create_root(parent, durability)?;
    }
    // Refuse an unavailable required parent barrier before publishing a directory.
    // Keep this descriptor through creation: only pre-existing trusted ancestors
    // may have a traverse-only parent that this process cannot open for reading.
    let parent = File::open(parent(path))?;
    durability.sync_directory(&parent)?;
    fs::DirBuilder::new().mode(0o700).create(path)?;
    durability.sync_directory(&File::open(path)?)?;
    durability.sync_directory(&parent)?;
    Ok(())
}
fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}
fn sync_root_parent(path: &Path, durability: &dyn Durability) -> std::io::Result<()> {
    durability.sync_directory(&File::open(parent(path))?)
}

pub(crate) use comb_protocol::fs::{check_file, check_name, read};

#[cfg(test)]
#[path = "fs/tests.rs"]
mod tests;
