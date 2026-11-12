//! Descriptor-bound source reads and private, durable snapshot publication.
use super::{Identity, Result, invalid, snapshot_path};
mod exclusions;
use crate::{error::Error, state};
pub(super) use exclusions::Exclusions;
use rustix::fs::{CWD, Mode, OFlags, RenameFlags, mkdirat, open, openat, renameat_with};
use std::{
    fs::{self, File, Metadata},
    os::unix::fs::MetadataExt,
    path::{Component, Path},
};

impl From<&Metadata> for Identity {
    fn from(m: &Metadata) -> Self {
        Self {
            device: m.dev(),
            inode: m.ino(),
            bytes: m.len(),
            mtime_seconds: m.mtime(),
            mtime_nanoseconds: m.mtime_nsec(),
            ctime_seconds: m.ctime(),
            ctime_nanoseconds: m.ctime_nsec(),
        }
    }
}
pub(super) struct SourceFile {
    pub file: File,
    pub path: std::path::PathBuf,
    pub observed: Identity,
}
impl SourceFile {
    pub(super) fn open(path: &Path, excluded: &Exclusions) -> Result<Self> {
        Self::open_checked(path, excluded, || {})
    }
    fn open_checked(
        path: &Path,
        excluded: &Exclusions,
        before_open: impl FnOnce(),
    ) -> Result<Self> {
        if !path.is_absolute() {
            return Err(invalid(path, "an absolute path is required."));
        }
        let canonical = path
            .canonicalize()
            .map_err(|e| invalid(path, "cannot resolve source.").with_source(e))?;
        if excluded.contains_path(&canonical) {
            return Err(invalid(path, "private Hive state cannot be attached."));
        }
        let before = fs::metadata(&canonical)
            .map_err(|e| invalid(path, "cannot inspect source.").with_source(e))?;
        check_source(path, &before)?;
        before_open();
        let file = open_canonical(&canonical, &excluded.identities)
            .map_err(|e| invalid(path, "cannot open source safely.").with_source(e))?;
        let opened = file.metadata().map_err(Error::state_io_from)?;
        check_source(path, &opened)?;
        if Identity::from(&before) != Identity::from(&opened) {
            return Err(invalid(path, "source changed while opening."));
        }
        Ok(Self {
            file,
            path: canonical,
            observed: Identity::from(&opened),
        })
    }
    pub(super) fn unchanged(&self) -> Result<()> {
        let now = self.file.metadata().map_err(Error::state_io_from)?;
        let named = fs::symlink_metadata(&self.path).map_err(Error::state_io_from)?;
        check_source(&self.path, &now)?;
        if !named.is_file()
            || Identity::from(&now) != self.observed
            || Identity::from(&named) != self.observed
            || self.path.canonicalize().map_err(Error::state_io_from)? != self.path
        {
            return Err(invalid(&self.path, "source changed during capture."));
        }
        Ok(())
    }
}
fn check_source(path: &Path, m: &Metadata) -> Result<()> {
    if !m.is_file() {
        return Err(invalid(path, "source must be a regular file."));
    }
    if m.uid() == crate::os::effective_uid() && m.nlink() != 1 {
        return Err(invalid(path, "owned hard links can alias private state."));
    }
    Ok(())
}
fn open_canonical(path: &Path, excluded: &[(u64, u64)]) -> std::io::Result<File> {
    // Walk the already canonical path without following ANY component symlink.
    // Bind the final descriptor to the metadata captured before this walk.
    let mut directory = File::from(open("/", search_flags(), Mode::empty())?);
    let mut components = path
        .components()
        .filter_map(|c| {
            if let Component::Normal(n) = c {
                Some(n)
            } else {
                None
            }
        })
        .peekable();
    while let Some(name) = components.next() {
        let metadata = directory.metadata()?;
        if excluded.contains(&(metadata.dev(), metadata.ino())) {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        let flags = if components.peek().is_some() {
            search_flags()
        } else {
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK
        };
        directory = File::from(openat(&directory, name, flags, Mode::empty())?);
    }
    Ok(directory)
}
fn search_flags() -> OFlags {
    #[cfg(target_os = "macos")]
    let access = OFlags::from_bits_retain(libc::O_SEARCH.cast_unsigned());
    #[cfg(not(target_os = "macos"))]
    let access = OFlags::PATH;
    access | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}
pub(super) fn publish(root: &Path, key: &str, name: &str, temp: tempfile::TempPath) -> Result<()> {
    use super::layout::{Directory, Payload, boundary, invalid};
    use atomic_fs::Step;
    let target = snapshot_path(root, key, name)?;
    let root = Directory::open(root)?;
    // The checked root descriptor anchors creation. Its existing post-publication
    // sync below covers this key entry too, including retries after failed creation.
    match mkdirat(&root.file, key, Mode::from_bits_truncate(0o700)) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(error) => return Err(Error::state_io_from(error.into())),
    }
    let directory = root.child(key)?.ok_or_else(invalid)?;
    let existing = directory.entry(Some(name))?;
    let mut staged = Payload {
        name: name.to_owned(),
        file: state::private_reader(&temp)?,
    };
    boundary(&target, Step::BeforePublish)?;
    root.check()?;
    directory.check_entry(existing.as_ref())?;
    let published = match renameat_with(CWD, &*temp, &directory.file, name, RenameFlags::NOREPLACE)
    {
        Ok(()) => staged,
        Err(error) if error == rustix::io::Errno::EXIST => {
            // A hash collision or damaged dedup target must never silently change a snapshot.
            let mut existing = existing.ok_or_else(invalid)?;
            if !equal(&mut existing.file, &mut staged.file)? {
                return Err(Error::state_inconsistent(
                    "Stored attachment content differs from its key.",
                ));
            }
            existing
        }
        Err(error) => return Err(Error::state_io_from(error.into())),
    };
    boundary(&target, Step::AfterPublish)?;
    root.check()?;
    directory.check_entry(Some(&published))?;
    directory.sync()?;
    root.sync()?;
    directory.check_entry(Some(&published))
}
fn equal(a: &mut File, b: &mut File) -> Result<bool> {
    use std::io::Read;
    if a.metadata().map_err(Error::state_io_from)?.len()
        != b.metadata().map_err(Error::state_io_from)?.len()
    {
        return Ok(false);
    }
    let mut left = vec![0; 65536];
    let mut right = vec![0; 65536];
    loop {
        let n = a.read(&mut left).map_err(Error::state_io_from)?;
        b.read_exact(&mut right[..n])
            .map_err(Error::state_io_from)?;
        if left[..n] != right[..n] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}
pub(in crate::runtime) fn sweep_staging(root: &Path) -> Result<()> {
    for entry in fs::read_dir(root).map_err(Error::state_io_from)? {
        let entry = entry.map_err(Error::state_io_from)?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|s| s.starts_with(".capture-"))
        {
            state::check_path(&entry.path(), false)?;
            fs::remove_file(entry.path()).map_err(Error::state_io_from)?;
        }
    }
    state::sync_directory(root).map_err(Error::state_io_from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::attachments::remove_snapshot;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn excluded_directory_identity_is_refused_without_a_matching_path_prefix() {
        let temp = crate::runtime::test_directory();
        let private = temp.path().join("private");
        let public = temp.path().join("public");
        state::directory(&private).unwrap();
        state::directory(&public).unwrap();
        let source = public.join("file");
        fs::write(&source, b"never captured").unwrap();
        let mut excluded = Exclusions::new(&private, &private).unwrap();
        let directory = open_canonical(&public.canonicalize().unwrap(), &[]).unwrap();
        let metadata = directory.metadata().unwrap();
        excluded.identities[0] = (metadata.dev(), metadata.ino());
        assert!(!excluded.contains_path(&source.canonicalize().unwrap()));
        assert_eq!(
            SourceFile::open(&source, &excluded).err().unwrap().code(),
            "attachment_invalid"
        );
        assert_eq!(fs::read_dir(&private).unwrap().count(), 0);
    }
    #[test]
    fn readable_sources_need_only_search_permission_on_ancestors() {
        let temp = crate::runtime::test_directory();
        let directory = temp.path().join("search-only");
        fs::create_dir(&directory).unwrap();
        let source = directory.join("file");
        fs::write(&source, "readable").unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o111)).unwrap();
        let private = temp.path().join("private");
        state::directory(&private).unwrap();
        let excluded = Exclusions::new(&private, &private).unwrap();
        let result = SourceFile::open(&source, &excluded);
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(result.unwrap().observed.bytes, 8);
    }
    #[test]
    fn atomic_publication_never_leaves_a_second_link() {
        use std::io::Write;
        let temp = crate::runtime::test_directory();
        let root = temp.path().join("snapshots");
        state::directory(&root).unwrap();
        let mut staged = tempfile::Builder::new()
            .prefix(".capture-")
            .tempfile_in(&root)
            .unwrap();
        staged.write_all(b"snapshot").unwrap();
        staged.as_file().sync_all().unwrap();
        let staged = staged.into_temp_path();
        let key = "a".repeat(64);
        let directory = root.join(&key);
        state::directory(&directory).unwrap();
        let target = directory.join("file");
        // Simulate death immediately after the publication syscall, before cleanup/fsync.
        renameat_with(CWD, &*staged, CWD, &target, RenameFlags::NOREPLACE).unwrap();
        assert!(!staged.exists());
        assert_eq!(fs::metadata(&target).unwrap().nlink(), 1);
        sweep_staging(&root).unwrap();
        remove_snapshot(&root, &key, "file").unwrap();
        assert!(!target.exists());
    }
    #[test]
    fn an_ancestor_symlink_swap_cannot_open_private_state() {
        let temp = crate::runtime::test_directory();
        let private = temp.path().join("private");
        fs::create_dir(&private).unwrap();
        let public = temp.path().join("public");
        fs::create_dir(&public).unwrap();
        fs::write(private.join("file"), "secret").unwrap();
        fs::write(public.join("file"), "safe").unwrap();
        let result = SourceFile::open_checked(
            &public.join("file"),
            &Exclusions::new(&private, &private).unwrap(),
            || {
                fs::rename(&public, temp.path().join("old")).unwrap();
                std::os::unix::fs::symlink(&private, &public).unwrap();
            },
        );
        assert_eq!(result.err().unwrap().code(), "attachment_invalid");
    }
}
