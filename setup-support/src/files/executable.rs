//! Executable discovery skips unusable ambient PATH entries and preserves storage failures.
use crate::{Result, invalid};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// Discover an optional executable; only a missing executable means absence.
pub fn optional_executable(path: &Path) -> Result<Option<PathBuf>> {
    match executable(path) {
        Ok(path) => Ok(Some(path)),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(cause) => Err(cause),
    }
}

/// Resolve ancestors but retain the installed executable's stable leaf symlink.
pub fn executable(path: &Path) -> Result<PathBuf> {
    let path = if path.components().count() == 1 {
        find(
            path,
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
            |candidate| fs::metadata(candidate),
        )?
    } else {
        path.to_owned()
    };
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Executable needs a directory."))?
        .canonicalize()?;
    let path = parent.join(
        path.file_name()
            .ok_or_else(|| invalid("Executable needs a name."))?,
    );
    let meta = fs::metadata(&path)?;
    if !meta.is_file() || meta.mode() & 0o111 == 0 {
        return Err(invalid("Program is not executable."));
    }
    Ok(path)
}

fn find(
    path: &Path,
    directories: impl Iterator<Item = PathBuf>,
    mut metadata: impl FnMut(&Path) -> Result<fs::Metadata>,
) -> Result<PathBuf> {
    for directory in directories {
        let candidate = directory.join(path);
        match metadata(&candidate) {
            Ok(meta) if meta.is_file() && meta.mode() & 0o111 != 0 => return Ok(candidate),
            Ok(_) => (),
            Err(cause)
                if matches!(
                    cause.kind(),
                    std::io::ErrorKind::NotFound
                        | std::io::ErrorKind::NotADirectory
                        | std::io::ErrorKind::PermissionDenied
                ) => {}
            Err(cause) => return Err(cause),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "Executable not found.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ambient_lookup_skips_denied_entries_but_never_io_or_resource_failures() {
        for errno in [libc::EACCES, libc::ENOTDIR, libc::EIO, libc::EMFILE] {
            for name in ["sh", "hive-unavailable-provider"] {
                let mut observed = Vec::new();
                let result = find(
                    Path::new(name),
                    ["/denied", "/bin"].into_iter().map(PathBuf::from),
                    |path| {
                        observed.push(path.to_owned());
                        if path.starts_with("/denied") {
                            Err(std::io::Error::from_raw_os_error(errno))
                        } else {
                            fs::metadata(path)
                        }
                    },
                );
                if errno == libc::EIO || errno == libc::EMFILE {
                    assert_eq!(result.unwrap_err().raw_os_error(), Some(errno));
                    assert_eq!(observed.len(), 1);
                } else if name == "sh" {
                    assert_eq!(result.unwrap(), Path::new("/bin/sh"));
                } else {
                    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::NotFound);
                }
            }
        }
        assert!(
            optional_executable(Path::new("/missing-hive-optional/program"))
                .unwrap()
                .is_none()
        );
    }
}
