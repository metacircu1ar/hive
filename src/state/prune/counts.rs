//! No-follow, descriptor-relative counts captured before native data deletion.
use crate::{
    error::{Error, Result},
    state,
};
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, fstat, openat, statat};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeData {
    pub files: u64,
    pub directories: u64,
    pub symlinks: u64,
    pub special_files: u64,
    pub bytes: u64,
}
impl NativeData {
    pub(super) fn capture(generation: &Path) -> Result<Self> {
        let mut counts = Self::default();
        for name in ["provider", "work"] {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
                .open(generation.join(name))
                .map_err(Error::state_io_from)?;
            state::check_metadata(&file.metadata().map_err(Error::state_io_from)?, true)?;
            counts.tree(&file)?;
        }
        Ok(counts)
    }
    fn tree(&mut self, directory: &File) -> Result<()> {
        add(&mut self.directories, 1)?;
        let entries = Dir::read_from(directory)
            .map_err(std::io::Error::from)
            .map_err(Error::state_io_from)?;
        for entry in entries {
            let entry = entry
                .map_err(std::io::Error::from)
                .map_err(Error::state_io_from)?;
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let meta = statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(std::io::Error::from)
                .map_err(Error::state_io_from)?;
            match FileType::from_raw_mode(meta.st_mode) {
                FileType::RegularFile => {
                    add(&mut self.files, 1)?;
                    add(
                        &mut self.bytes,
                        u64::try_from(meta.st_size)
                            .map_err(|cause| state::corrupt().with_source(cause))?,
                    )?;
                }
                FileType::Symlink => add(&mut self.symlinks, 1)?,
                FileType::Directory => {
                    let child = openat(
                        directory,
                        name,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
                        Mode::empty(),
                    )
                    .map_err(std::io::Error::from)
                    .map_err(Error::state_io_from)?;
                    let held = fstat(&child)
                        .map_err(std::io::Error::from)
                        .map_err(Error::state_io_from)?;
                    if (held.st_dev, held.st_ino) != (meta.st_dev, meta.st_ino) {
                        return Err(state::corrupt());
                    }
                    self.tree(&File::from(child))?;
                }
                _ => add(&mut self.special_files, 1)?,
            }
        }
        Ok(())
    }
}
fn add(count: &mut u64, amount: u64) -> Result<()> {
    *count = count.checked_add(amount).ok_or_else(state::corrupt)?;
    Ok(())
}
