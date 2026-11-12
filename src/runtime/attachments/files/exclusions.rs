//! Bind private roots to directory identities, including non-symlink aliases.
use super::{Error, Path, Result, open_canonical};
use std::{os::unix::fs::MetadataExt, path::PathBuf};

pub(in crate::runtime::attachments) struct Exclusions {
    paths: [PathBuf; 2],
    pub(super) identities: [(u64, u64); 2],
}
impl Exclusions {
    pub(in crate::runtime::attachments) fn new(private: &Path, snapshots: &Path) -> Result<Self> {
        let paths = [
            private.canonicalize().map_err(Error::state_io_from)?,
            snapshots.canonicalize().map_err(Error::state_io_from)?,
        ];
        let mut identities = [(0, 0); 2];
        for (path, identity) in paths.iter().zip(&mut identities) {
            let directory = open_canonical(path, &[]).map_err(Error::state_io_from)?;
            let metadata = directory.metadata().map_err(Error::state_io_from)?;
            if !metadata.is_dir() {
                return Err(Error::unsafe_state(
                    "Attachment directory contents or ownership changed.",
                ));
            }
            *identity = (metadata.dev(), metadata.ino());
        }
        Ok(Self { paths, identities })
    }

    pub(super) fn contains_path(&self, path: &Path) -> bool {
        self.paths.iter().any(|root| path.starts_with(root))
    }
}
