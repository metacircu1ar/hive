//! Compare executable identity without retaining a native process or catalog.
use crate::error::{Error, Result};
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BinaryIdentity {
    path: PathBuf,
    device: u64,
    inode: u64,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl BinaryIdentity {
    pub(crate) fn read(path: &Path) -> Result<Self> {
        let path = path.canonicalize().map_err(Error::state_io_from)?;
        let meta = path.metadata().map_err(Error::state_io_from)?;
        Ok(Self {
            path,
            device: meta.dev(),
            inode: meta.ino(),
            length: meta.len(),
            modified: (meta.mtime(), meta.mtime_nsec()),
            changed: (meta.ctime(), meta.ctime_nsec()),
        })
    }
    /// Compare only a persisted proof; callers must also verify its content digest.
    pub(crate) fn matches_saved(&self, live: &Self) -> Result<bool> {
        if self == live {
            return Ok(true);
        }
        Ok(self.path == live.path
            && self.inode == live.inode
            && self.length == live.length
            && self.modified == live.modified
            && self.changed == live.changed
            && hive_setup_support::files::identity::saved(
                (self.device, self.inode),
                (live.device, live.inode),
                hive_setup_support::files::identity::parent_device(&live.path)
                    .map_err(Error::state_io_from)?,
            )
            .is_match())
    }
    pub(crate) fn unchanged(&self) -> bool {
        Self::read(&self.path)
            .inspect_err(|error| {
                crate::observability::observation_failed("binary_identity_unchanged", error);
            })
            .is_ok_and(|current| current == *self)
    }
}
