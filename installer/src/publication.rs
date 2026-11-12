//! Complete installer files; callers retain ownership and generation decisions.
use crate::Result;
use atomic_fs::{Barriers, Options, PreparedCreate};
use std::{fs, io, os::unix::fs::DirBuilderExt, path::Path};
mod directory;
pub(crate) use directory::Owned;
#[cfg(test)]
pub(crate) use directory::tests::with_creation_hook;
mod image;
pub(crate) use image::Image;

fn retains(error: &(dyn std::error::Error + 'static), depth: usize) -> bool {
    if depth == 32 {
        return true;
    }
    if let Some(owner) = error.downcast_ref::<crate::Error>() {
        return retains(owner.as_ref(), depth + 1);
    }
    if error.is::<crate::setup::RetainedFailure>() {
        return true;
    }
    if let Some(publication) = error.downcast_ref::<atomic_fs::Error>()
        && (publication.published()
            || publication.retained().is_some()
            || publication.cleanup_error().is_some())
    {
        return true;
    }
    if let Some(cleanup) = error.downcast_ref::<CleanupFailure>() {
        return retains(cleanup.primary.as_ref(), depth + 1)
            || retains(cleanup.cleanup.as_ref(), depth + 1);
    }
    if let Some(native) = error
        .downcast_ref::<io::Error>()
        .and_then(io::Error::get_ref)
    {
        return retains(native, depth + 1);
    }
    error
        .source()
        .is_some_and(|source| retains(source, depth + 1))
}

pub(crate) fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_fs::Publisher::new(options(0o644)).create_new(path, bytes)?;
    Ok(())
}

fn options(mode: u32) -> Options<'static> {
    (hive_setup_support::names::PUBLICATION_OPTIONS).with_mode(mode)
}

pub(crate) fn copy(source: &Path, destination: &Path, mode: u32) -> Result<()> {
    let mut input = crate::ownership::regular(source)?;
    let size = input.metadata()?.len();
    stream(
        destination,
        mode,
        &mut input,
        size,
        &atomic_fs::DefaultBarriers,
    )?;
    Ok(())
}

pub(crate) fn stream(
    destination: &Path,
    mode: u32,
    input: &mut impl io::Read,
    size: u64,
    barriers: &impl Barriers,
) -> atomic_fs::Result<u64> {
    PreparedCreate::new(destination, options(mode))?.publish_stream(
        |output| {
            let count = io::copy(input, output)?;
            if count != size {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Payload length changed",
                ));
            }
            Ok(count)
        },
        barriers,
    )
}

pub(crate) fn manager(path: &Path, hook: &mut impl FnMut(&str) -> Result<()>) -> Result<()> {
    struct Hooks<'a, F>(&'a std::cell::RefCell<F>);
    impl<F: FnMut(&str) -> Result<()>> Barriers for Hooks<'_, F> {
        fn observe(&self, step: atomic_fs::Step) -> io::Result<()> {
            let name = match step {
                atomic_fs::Step::BeforePublish => "manager_before_publish",
                atomic_fs::Step::AfterPublish => "manager_after_publish",
                atomic_fs::Step::BeforeDirectorySync => "manager_before_sync",
                atomic_fs::Step::AfterDirectorySync => "manager_after_sync",
                _ => return Ok(()),
            };
            (self.0.borrow_mut())(name).map_err(crate::Error::into_io_other)
        }
    }
    let hook = std::cell::RefCell::new(hook);
    atomic_fs::Publisher::new(options(0o700))
        .with_barriers(&Hooks(&hook))
        .staged_dir(path, |stage| {
            atomic_fs::Publisher::new(options(0o644))
                .create_new(stage.join("owner"), crate::publish::OWNER)
                .map_err(io::Error::other)?;
            fs::DirBuilder::new()
                .mode(0o700)
                .create(stage.join("generations"))
        })?;
    Ok(())
}

/// A failed prepare/rollback cannot discard an image still named by a journal.
pub(crate) fn discard_unreferenced(manager: &Path, generation: &Path) -> Result<()> {
    let pending = match fs::symlink_metadata(manager.join("pending-install.json")) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    if pending || crate::publish::current(manager)?.is_some_and(|p| manager.join(p) == generation) {
        return Ok(());
    }
    fs::remove_dir_all(generation)?;
    crate::publish::sync(&manager.join("generations"))
}

/// Preserve the original error object, including publication/retention evidence.
#[derive(Debug)]
pub(crate) struct CleanupFailure {
    primary: crate::Error,
    cleanup: crate::Error,
    action: &'static str,
}
impl std::fmt::Display for CleanupFailure {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "{}. {}: {}; recovery evidence retained",
            self.primary, self.action, self.cleanup
        )
    }
}
impl std::error::Error for CleanupFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.primary.as_ref())
    }
}
pub(crate) fn failed(
    primary: crate::Error,
    action: &'static str,
    cleanup: Result<()>,
) -> crate::Error {
    match cleanup {
        Ok(()) => {
            tracing::info!(name: "install.cleanup_finished",
                 success = true,
                 decision = "preserve_primary_failure");
            primary
        }
        Err(cleanup) => {
            tracing::error!(name: "install.cleanup_failed",
                 primary = primary.as_ref() as &(dyn std::error::Error + 'static),
                 secondary = cleanup.as_ref() as &(dyn std::error::Error + 'static),
                 decision = "retain_recovery_evidence");
            CleanupFailure {
                primary,
                cleanup,
                action,
            }
            .into()
        }
    }
}

#[derive(Debug)]
pub(crate) struct ContextFailure {
    action: &'static str,
    cause: crate::Error,
}
impl std::fmt::Display for ContextFailure {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{}: {}", self.action, self.cause)
    }
}
impl std::error::Error for ContextFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}
pub(crate) fn context(action: &'static str, cause: crate::Error) -> crate::Error {
    ContextFailure { action, cause }.into()
}

#[cfg(test)]
mod tests;
