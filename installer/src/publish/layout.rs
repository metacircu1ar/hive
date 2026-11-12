//! Installation-manager admission and ownership.
use super::{Result, manifest, publication};
use crate::ownership::{self, Directory};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
pub(crate) const OWNER: &[u8] = b"Hive managed installation\n";

pub(super) fn directory(path: &Path, hook: &mut impl FnMut(&str) -> Result<()>) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => ownership::routes(path),
        Ok(_) => Err(format!(
            "Refusing non-directory installation path: {}",
            path.display()
        )
        .into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ownership::routes(path)?;
            fs::DirBuilder::new().mode(0o755).create(path)?;
            sync_parent(path, hook)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

pub(super) fn sync_parent(path: &Path, hook: &mut impl FnMut(&str) -> Result<()>) -> Result<()> {
    let parent = path
        .parent()
        .ok_or("Installation directory has no parent")?;
    sync(parent)?;
    hook(&format!("parent_synced:{}", parent.display()))
}

pub(super) fn prefix_directories(
    prefix: &Path,
    hook: &mut impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    if !prefix.is_absolute() || prefix.is_symlink() {
        return Err("Installation prefix must be absolute and not a symlink".into());
    }
    ownership::routes(prefix)?;
    let existing = prefix
        .ancestors()
        .find(|path| path.is_dir())
        .ok_or("Installation prefix has no existing ancestor")?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(prefix)?;
    ownership::routes(prefix)?;
    // Also cover ancestors from an interrupted first attempt. Read-only mounts
    // cannot contain directory entries newly created by this installer.
    for parent in prefix.ancestors().skip(1) {
        match fs::File::open(parent).and_then(|file| file.sync_all()) {
            Ok(()) => hook(&format!("parent_synced:{}", parent.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::ReadOnlyFilesystem => {}
            // Traversable ancestors may deny listing/opening. This invocation
            // created no entry above the nearest previously existing directory.
            Err(error)
                if error.kind() == std::io::ErrorKind::PermissionDenied
                    && parent != existing
                    && existing.starts_with(parent) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(crate) fn sync(path: &Path) -> Result<()> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

pub(crate) fn manager(prefix: &Path, hook: &mut impl FnMut(&str) -> Result<()>) -> Result<PathBuf> {
    if !prefix.is_absolute() || prefix.is_symlink() {
        return Err("Installation prefix must be absolute and not a symlink".into());
    }
    prefix_directories(prefix, hook)?;
    directory(&prefix.join("lib"), hook)?;
    let manager = prefix.join("lib/hive");
    match fs::symlink_metadata(&manager) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            publication::manager(&manager, hook)?;
        }
        Ok(_) => inspect_existing(prefix)?,
        Err(error) => return Err(error.into()),
    }
    // Publication can have succeeded before its parent barrier failed. Reuse
    // repeats the complete-manager barrier before any generation is selected.
    sync(&manager.join("owner"))?;
    sync(&manager.join("generations"))?;
    sync_parent(&manager.join("generations"), hook)?;
    sync_parent(&manager, hook)?;
    Ok(manager)
}
pub(crate) fn inspect_existing(prefix: &Path) -> Result<()> {
    ownership::routes(prefix)?;
    for name in ["lib", "bin", "share"] {
        let path = prefix.join(name);
        if hive_setup_support::files::optional_metadata(&path)?.is_some() {
            ownership::routes(&path)?;
        }
    }
    let manager = prefix.join("lib/hive");
    if hive_setup_support::files::optional_metadata(&manager)?.is_some() {
        ownership::directory(&manager, Directory::Manager)?;
        ownership::owner_record(&manager.join("owner"), OWNER)?;
        ownership::directory(&manager.join("generations"), Directory::Inner)?;
    }
    Ok(())
}

pub(crate) fn current(manager: &Path) -> Result<Option<PathBuf>> {
    match fs::read_link(manager.join("current")) {
        Ok(path) => {
            let parts: Vec<_> = path.components().collect();
            if parts.len() != 2
                || !manifest::safe_path(path.to_str().ok_or("Invalid current path")?)
                || parts[0].as_os_str() != "generations"
                || !parts[1].as_os_str().to_string_lossy().starts_with("g-")
            {
                return Err("Invalid owned current pointer".into());
            }
            if !fs::symlink_metadata(manager.join(&path))?.is_dir() {
                return Err("Current generation is not a real directory".into());
            }
            Ok(Some(path))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn installation_lock(manager: &Path) -> Result<hive_setup_support::locks::Guard> {
    inspect_existing(
        manager
            .parent()
            .and_then(Path::parent)
            .ok_or("Missing installation prefix")?,
    )?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().try_into()?)
        .open(
            manager
                .parent()
                .ok_or("Missing installation parent")?
                .join(".hive-install.lock"),
        )?;
    let metadata = lock.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.nlink() != 1
        || metadata.mode() & 0o022 != 0
    {
        return Err("Unsafe installation lock".into());
    }
    let guard = hive_setup_support::locks::Guard::exclusive(lock).map_err(|error| {
        publication::context("Cannot acquire installation ownership", error.into())
    })?;
    Ok(guard)
}
