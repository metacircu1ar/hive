//! Exact object receipts for retryable deletion under product-owned locks.
use crate::{Result, files, invalid};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    device: u64,
    inode: u64,
    kind: u32,
}
impl Identity {
    /// Compare this persisted receipt with a freshly validated object at `path`.
    /// Callers must retain their namespace checks and exact live race checks.
    pub fn matches_saved(&self, live: &Self, path: &Path) -> Result<bool> {
        if self == live {
            return Ok(true);
        }
        Ok(self.kind == live.kind
            && self.inode == live.inode
            && files::identity::saved(
                (self.device, self.inode),
                (live.device, live.inode),
                files::identity::parent_device(path)?,
            )
            .is_match())
    }
    pub fn is_regular_file(&self) -> bool {
        self.kind == 0o100_000
    }
    pub fn read(path: &Path) -> Result<Option<Self>> {
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if meta.file_type().is_symlink() || meta.uid() != rustix::process::geteuid().as_raw() {
            return Err(invalid(format!(
                "Refusing unowned or symlinked purge object: {}",
                path.display()
            )));
        }
        Ok(Some(Self {
            device: meta.dev(),
            inode: meta.ino(),
            kind: meta.mode() & 0o170_000,
        }))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Object {
    pub path: PathBuf,
    #[serde(deserialize_with = "hive_input::decode::optional_object")]
    pub identity: Option<Identity>,
}
impl Object {
    pub fn capture(path: &Path) -> Result<Self> {
        let path = safe_path(path)?;
        Ok(Self {
            identity: Identity::read(&path)?,
            path,
        })
    }
    pub fn check(&self) -> Result<()> {
        let same = match (&self.identity, Identity::read(&self.path)?) {
            (Some(saved), Some(live)) => saved.matches_saved(&live, &self.path)?,
            (None, None) => true,
            _ => false,
        };
        if safe_path(&self.path)? != self.path || !same {
            return Err(invalid(format!(
                "Purge object changed since admission; refusing {}",
                self.path.display()
            )));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    #[serde(deserialize_with = "hive_input::decode::object")]
    pub object: Object,
    pub quarantine: PathBuf,
    pub done: bool,
}
impl Action {
    pub fn capture(path: &Path, nonce: &str) -> Result<Self> {
        if nonce.is_empty()
            || !nonce
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(invalid("Invalid purge identity."));
        }
        let object = Object::capture(path)?;
        let name = object
            .path
            .file_name()
            .ok_or_else(|| invalid("Purge object needs a name."))?
            .to_string_lossy();
        let quarantine = object
            .path
            .with_file_name(format!(".{name}.hive-purge-{nonce}"));
        if files::optional_metadata(&quarantine)?.is_some() {
            return Err(invalid("Purge quarantine already exists."));
        }
        Ok(Self {
            object,
            quarantine,
            done: false,
        })
    }
    /// The admitted object identity is also the quarantine identity after rename.
    /// Persist the complete receipt before calling this, and after marking done.
    pub fn remove(&mut self) -> Result<()> {
        self.remove_tracking(&mut false)
    }
    pub fn remove_tracking(&mut self, changed: &mut bool) -> Result<()> {
        self.remove_with(rename_noreplace, changed)
    }
    fn remove_with(
        &mut self,
        rename: impl FnOnce(&Path, &Path) -> Result<()>,
        changed: &mut bool,
    ) -> Result<()> {
        self.validate()?;
        let current = Identity::read(&self.object.path)?;
        let quarantined = Identity::read(&self.quarantine)?;
        if self.done {
            if current.is_some() || quarantined.is_some() {
                return Err(invalid(
                    "A completed purge path was recreated; refusing a second deletion.",
                ));
            }
            return Ok(());
        }
        if self.object.identity.is_none() {
            self.object.check()?;
            if quarantined.is_some() {
                return Err(invalid("Unadmitted quarantine object."));
            }
        } else if quarantined.is_some() {
            if current.is_some()
                || !self
                    .object
                    .identity
                    .as_ref()
                    .zip(quarantined.as_ref())
                    .map(|(saved, live)| saved.matches_saved(live, &self.quarantine))
                    .transpose()?
                    .unwrap_or(false)
            {
                return Err(invalid("Purge quarantine or original path was replaced."));
            }
            remove_object(&self.quarantine, changed)?;
        } else if current.is_some() {
            self.object.check()?;
            rename(&self.object.path, &self.quarantine)?;
            *changed = true;
            files::sync_parent(&self.quarantine)?;
            if Identity::read(&self.quarantine)? != current {
                return Err(invalid("Purge quarantine identity changed."));
            }
            remove_object(&self.quarantine, changed)?;
        }
        if let Some(parent) = self.object.path.parent()
            && parent.try_exists()?
        {
            files::sync_parent(&self.object.path)?;
        }
        self.done = true;
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        if safe_path(&self.object.path)? != self.object.path
            || self.quarantine.parent() != self.object.path.parent()
            || !self.quarantine.file_name().is_some_and(|n| {
                n.to_string_lossy().starts_with(&format!(
                    ".{}.hive-purge-",
                    self.object
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                ))
            })
        {
            return Err(invalid("Invalid purge action scope."));
        }
        Ok(())
    }
}
/// Fail closed if the filesystem cannot atomically reserve an unoccupied name.
pub fn rename_noreplace(source: &Path, destination: &Path) -> Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE)
        .map_err(std::io::Error::from)
}
fn remove_object(path: &Path, changed: &mut bool) -> Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        // An admitted recursive removal can fail after removing some descendants.
        *changed = true;
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
        *changed = true;
    }
    files::sync_parent(path)
}
pub fn safe_path(path: &Path) -> Result<PathBuf> {
    let path = files::absolute(path)?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|p| files::location(&p))
        .transpose()?;
    if path.parent().is_none()
        || path.components().count() < 3
        || home
            .as_ref()
            .is_some_and(|h| h == &path || h.starts_with(&path))
        || [
            "/usr",
            "/usr/local",
            "/opt",
            "/tmp",
            "/var",
            "/Users",
            "/home",
        ]
        .iter()
        .any(|p| path == Path::new(p))
    {
        return Err(invalid(format!(
            "Refusing broad purge path: {}",
            path.display()
        )));
    }
    Ok(path)
}
pub fn disjoint(paths: &[&Path]) -> Result<()> {
    for (i, path) in paths.iter().enumerate() {
        for other in &paths[i + 1..] {
            if path.starts_with(other) || other.starts_with(path) {
                return Err(invalid(
                    "Purge paths overlap; select independent product locations.",
                ));
            }
        }
    }
    Ok(())
}
/// Validate an owned product subtree without following links or crossing devices.
pub fn tree(path: &Path) -> Result<()> {
    let Some(identity) = Identity::read(path)? else {
        return Ok(());
    };
    let mut pending = vec![path.to_owned()];
    while let Some(path) = pending.pop() {
        let next = Identity::read(&path)?
            .ok_or_else(|| invalid("Purge tree changed during inspection."))?;
        if next.device != identity.device {
            return Err(invalid("Purge cannot cross a mount point."));
        }
        let meta = fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            for entry in fs::read_dir(path)? {
                pending.push(entry?.path());
            }
        } else if !meta.is_file() && next.kind != 0o140_000 {
            return Err(invalid("Purge tree contains an unsupported object."));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_purge_identity_allows_renumbering_but_not_inode_or_kind_changes() {
        for (field, allowed) in [("device", true), ("inode", false), ("kind", false)] {
            for quarantined in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let path = temp.path().join("owned");
                files::write(&path, b"keep until admitted", false).unwrap();
                let action = Action::capture(&path, "renumber").unwrap();
                if quarantined {
                    fs::rename(&path, &action.quarantine).unwrap();
                }
                let mut value = serde_json::to_value(&action).unwrap();
                value["object"]["identity"][field] =
                    serde_json::json!(value["object"]["identity"][field].as_u64().unwrap() ^ 1);
                let mut saved: Action = serde_json::from_value(value.clone()).unwrap();
                assert_eq!(saved.remove().is_ok(), allowed, "{field}/{quarantined}");
                assert_eq!(
                    saved.object.identity,
                    serde_json::from_value::<Action>(value)
                        .unwrap()
                        .object
                        .identity
                );
                assert_eq!(path.exists() || action.quarantine.exists(), !allowed);
            }
        }
    }
    #[test]
    fn collision_at_rename_keeps_both_admitted_and_foreign_objects() {
        for directory in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("owned");
            if directory {
                fs::create_dir(&path).unwrap();
            } else {
                fs::write(&path, b"owned").unwrap();
            }
            let mut action = Action::capture(&path, "collision").unwrap();
            let admitted = action.object.identity.clone();
            let mut foreign = None;
            let error = action
                .remove_with(
                    |source, destination| {
                        if directory {
                            fs::create_dir(destination)?;
                        } else {
                            fs::write(destination, b"foreign")?;
                        }
                        foreign = Identity::read(destination)?;
                        rename_noreplace(source, destination)
                    },
                    &mut false,
                )
                .unwrap_err();
            assert!(matches!(
                error.raw_os_error(),
                Some(libc::EEXIST | libc::ENOTEMPTY)
            ));
            assert_eq!(Identity::read(&path).unwrap(), admitted);
            assert_eq!(Identity::read(&action.quarantine).unwrap(), foreign);
            if !directory {
                assert_eq!(fs::read(&path).unwrap(), b"owned");
                assert_eq!(fs::read(&action.quarantine).unwrap(), b"foreign");
            }
            assert!(!action.done);
        }
    }
    #[test]
    fn unsupported_noreplace_does_not_move_or_delete_the_object() {
        for errno in [libc::EINVAL, libc::ENOTSUP] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("owned");
            fs::write(&path, b"owned").unwrap();
            let mut action = Action::capture(&path, "unsupported").unwrap();
            let identity = action.object.identity.clone();
            let error = action
                .remove_with(
                    |_, _| Err(std::io::Error::from_raw_os_error(errno)),
                    &mut false,
                )
                .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(errno));
            assert_eq!(Identity::read(&path).unwrap(), identity);
            assert_eq!(fs::read(&path).unwrap(), b"owned");
            assert!(!action.quarantine.exists());
            assert!(!action.done);
        }
    }
    #[test]
    fn retry_removes_only_admitted_quarantine_and_refuses_recreation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("owned");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("data"), b"data").unwrap();
        let mut action = Action::capture(&path, "test").unwrap();
        fs::rename(&path, &action.quarantine).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(action.remove().is_err());
        fs::remove_dir(&path).unwrap();
        action.remove().unwrap();
        action.remove().unwrap();
        fs::create_dir(&path).unwrap();
        assert!(action.remove().is_err());
    }
    #[test]
    fn absent_admission_never_deletes_a_later_object() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("later");
        let mut action = Action::capture(&path, "absent").unwrap();
        fs::write(&path, b"keep").unwrap();
        assert!(action.remove().is_err());
        assert_eq!(fs::read(path).unwrap(), b"keep");
    }
}
