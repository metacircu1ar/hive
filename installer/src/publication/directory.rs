//! Retain uncertain preparations and never recursively clean a replaced directory.
use crate::{Error, Result};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use tempfile::TempDir;
#[cfg(test)]
pub(super) mod tests;

pub(crate) struct Owned {
    directory: Option<TempDir>,
    path: PathBuf,
    identity: fs::Metadata,
}
impl Owned {
    pub(crate) fn new(directory: TempDir) -> Result<Self> {
        let path = directory.path().to_owned();
        match crate::ownership::directory(&path, crate::ownership::Directory::Manager) {
            Ok(identity) => {
                let owned = Self {
                    directory: Some(directory),
                    path,
                    identity,
                };
                #[cfg(test)]
                if let Err(error) = tests::created(owned.path()) {
                    return owned.finish(Err(error.into()));
                }
                Ok(owned)
            }
            Err(error) => {
                let _retained = directory.keep();
                Err(retained(&path, error))
            }
        }
    }
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    fn check(&self) -> Result<()> {
        let observed =
            crate::ownership::directory(&self.path, crate::ownership::Directory::Manager)?;
        if (observed.dev(), observed.ino()) != (self.identity.dev(), self.identity.ino()) {
            return Err("Installer preparation identity changed; retain it unchanged".into());
        }
        Ok(())
    }
    fn retain(&mut self) {
        if let Some(directory) = self.directory.take() {
            let _retained = directory.keep();
        }
    }
    pub(crate) fn finish<T>(mut self, result: Result<T>) -> Result<T> {
        // Retention is transitive: an enclosing staging owner must see why this
        // directory cannot be cleaned, including after an otherwise successful run.
        if let Err(error) = self.check() {
            self.retain();
            let evidence = retained(&self.path, error);
            return Err(match result {
                Ok(_) => evidence,
                Err(primary) => super::failed(
                    primary,
                    "Installer preparation could not be confirmed",
                    Err(evidence),
                ),
            });
        }
        if let Err(error) = &result
            && super::retains(error.as_ref(), 0)
        {
            self.retain();
            tracing::warn!(name: "install.preparation_retained",
                decision = "retain_recovery_evidence");
        }
        result
    }
    pub(crate) fn keep(mut self) -> Result<PathBuf> {
        self.check()?;
        self.directory
            .take()
            .map(TempDir::keep)
            .ok_or_else(|| Error::from("Installer preparation is unavailable"))
    }
}
fn retained(path: &Path, cause: Error) -> Error {
    atomic_fs::Error::new(cause.into_io_other(), false)
        .with_retained(path)
        .into()
}
impl Drop for Owned {
    fn drop(&mut self) {
        if self.directory.is_none() {
            return;
        }
        if std::thread::panicking() {
            self.retain();
        } else if let Err(error) = self.check() {
            tracing::warn!(name: "install.preparation_cleanup_refused",
                error = error.as_ref() as &(dyn std::error::Error + 'static),
                decision = "retain_recovery_evidence");
            self.retain();
        }
    }
}
