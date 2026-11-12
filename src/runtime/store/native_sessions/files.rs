//! No-follow, identity-bound native file discovery and deletion.
mod discovery;
use crate::{
    config::Provider,
    error::{Error, Result},
    state,
};
use atomic_fs::{Barriers, Step};
pub(super) use discovery::{Budget, discover};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::ErrorKind,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

use crate::runtime::api::NativeWarningReason as Warning;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    device: u64,
    inode: u64,
    bytes: u64,
    modified_seconds: i64,
    modified_nanos: i64,
}
impl Identity {
    fn matches_saved(&self, metadata: &fs::Metadata, parent: &fs::Metadata) -> bool {
        let live = Self::of(metadata);
        self.bytes == live.bytes
            && self.modified_seconds == live.modified_seconds
            && self.modified_nanos == live.modified_nanos
            && hive_setup_support::files::identity::saved(
                (self.device, self.inode),
                (live.device, live.inode),
                parent.dev(),
            )
            .is_match()
    }
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanos: metadata.mtime_nsec(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct File {
    pub path: PathBuf,
    #[serde(deserialize_with = "crate::decode::object")]
    identity: Identity,
}
impl File {
    pub fn bytes(&self) -> u64 {
        self.identity.bytes
    }
}

#[derive(Clone, Default)]
pub(super) struct Plan {
    pub files: Vec<File>,
    pub warning: Option<Warning>,
}
pub(super) enum Removed {
    Deleted(u64),
    AlreadyAbsent,
    Retained(Warning),
}

struct Cleanup;
impl Barriers for Cleanup {
    #[cfg(test)]
    fn observe(&self, step: Step) -> std::io::Result<()> {
        tests::step(step)
    }
}
fn sync_directory(path: &Path) -> Result<()> {
    let sync = || -> std::io::Result<()> {
        Cleanup.observe(Step::BeforeDirectorySync)?;
        Cleanup.sync_directory(&fs::File::open(path)?)?;
        Cleanup.observe(Step::AfterDirectorySync)
    };
    sync().map_err(Error::state_io_from)
}

/// Each recorded home is derived from the fixed state root and canonical generation.
/// Validate all managed ancestors, including ones shared by native sessions.
fn home_metadata(home: &Path) -> Result<Option<fs::Metadata>> {
    let mut chain = Vec::new();
    let mut current = Some(home);
    for _ in 0..4 {
        let path = current.ok_or_else(invalid_path)?;
        chain.push(path);
        current = path.parent();
    }
    let mut result = None;
    for path in chain.into_iter().rev() {
        result = metadata(path, true)?;
        if result.is_none() {
            break;
        }
    }
    Ok(result)
}

fn invalid_path() -> Error {
    Error::database_error("Invalid native session cleanup path.")
}

fn metadata(path: &Path, directory: bool) -> Result<Option<fs::Metadata>> {
    let checked =
        if directory {
            state::checked_metadata(path, true)
        } else {
            fs::symlink_metadata(path).map_err(Error::state_io_from).and_then(|meta| {
            // Native checkpoint copies can deliberately preserve 0644/0755.
            // Deletion does not read them; private managed parents and identity
            // checks provide ownership, independently of a leaf's read mode.
            if !meta.is_file() || meta.uid() != crate::os::effective_uid() || meta.nlink() != 1 {
                Err(Error::unsafe_state(
                    "Native session cleanup requires owned regular files without symlinks or \
                    extra hard links.",
                ))
            } else {
                Ok(meta)
            }
        })
        };
    match checked {
        Ok(meta) => Ok(Some(meta)),
        Err(error) if error.io_kind() == Some(ErrorKind::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

fn canonical_session(session: &str) -> bool {
    matches!(uuid::Uuid::parse_str(session), Ok(id) if id.to_string() == session)
}

/// This allowlist also validates deserialized outbox paths before touching disk.
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "Delete only the native provider's exact lowercase filename pattern; never \
        expand the ownership allowlist."
)]
fn owned_path(provider: Provider, session: &str, path: &Path) -> bool {
    if !canonical_session(session) {
        return false;
    }
    let Some(parts): Option<Vec<_>> = path
        .components()
        .map(|part| match part {
            Component::Normal(text) => text.to_str(),
            _ => None,
        })
        .collect()
    else {
        return false;
    };
    match (provider, parts.as_slice()) {
        (Provider::Codex, _) => transcript_path(provider, session, path),
        (Provider::Claude, ["projects", _, name]) => *name == format!("{session}.jsonl"),
        (
            Provider::Claude,
            ["projects", _, id, _, ..] | ["file-history" | "session-env" | "tasks", id, _, ..],
        ) => *id == session,
        (Provider::Claude, ["debug", name]) => *name == format!("{session}.txt"),
        (Provider::Claude, ["todos", name]) => {
            name.starts_with(&format!("{session}-agent-")) && name.ends_with(".json")
        }
        _ => false,
    }
}

pub(super) fn transcript_path(provider: Provider, session: &str, path: &Path) -> bool {
    if !canonical_session(session) {
        return false;
    }
    let Some(parts): Option<Vec<_>> = path
        .components()
        .map(|part| match part {
            Component::Normal(text) => text.to_str(),
            _ => None,
        })
        .collect()
    else {
        return false;
    };
    let name = match (provider, parts.as_slice()) {
        (Provider::Claude, ["projects", project, name]) if project.len() <= 255 => {
            return *name == format!("{session}.jsonl");
        }
        (Provider::Codex, ["sessions", year, month, day, name])
            if digits(year, 4) && digits(month, 2) && digits(day, 2) =>
        {
            *name
        }
        (Provider::Codex, ["archived_sessions", name]) => *name,
        _ => return false,
    };
    if name.len() > 255 {
        return false;
    }
    name.strip_prefix("rollout-")
        .and_then(|name| name.strip_suffix(&format!("-{session}.jsonl")))
        .is_some_and(|timestamp| !timestamp.is_empty())
}
fn digits(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Return the nearest checked directory if part of the saved path is gone. It is
/// synced even on a retry after unlink, before acknowledging the durable outbox.
fn checked_parent(home: &Path, path: &Path) -> Result<(PathBuf, bool)> {
    let mut current = home.to_path_buf();
    let parent = path.parent().ok_or_else(invalid_path)?;
    for part in parent.components() {
        let next = current.join(part);
        if metadata(&next, true)?.is_none() {
            return Ok((current, false));
        }
        current = next;
    }
    Ok((current, true))
}

pub(super) fn unlink(
    home: &Path,
    provider: Provider,
    session: &str,
    file: &File,
) -> Result<Removed> {
    if !owned_path(provider, session, &file.path) {
        return Err(invalid_path());
    }
    match unlink_checked(home, file) {
        Err(Error::UnsafeState { .. }) => Ok(Removed::Retained(Warning::UnsafePath)),
        other => other,
    }
}

fn unlink_checked(home: &Path, file: &File) -> Result<Removed> {
    if home_metadata(home)?.is_none() {
        // Auth pruning may have removed the whole generation. Confirm the
        // absence beneath the nearest remaining managed directory too.
        for parent in home.ancestors().skip(1).take(3) {
            if metadata(parent, true)?.is_some() {
                sync_directory(parent)?;
                return Ok(Removed::AlreadyAbsent);
            }
        }
        return Err(invalid_path());
    }
    let (parent, present) = checked_parent(home, &file.path)?;
    let path = home.join(&file.path);
    let current = if present {
        metadata(&path, false)?
    } else {
        None
    };
    let outcome = match current {
        Some(meta)
            if !file.identity.matches_saved(
                &meta,
                &fs::symlink_metadata(&parent).map_err(Error::state_io_from)?,
            ) =>
        {
            return Ok(Removed::Retained(Warning::IdentityChanged));
        }
        Some(meta) => {
            let live = Identity::of(&meta);
            Cleanup
                .observe(Step::BeforeRemove)
                .map_err(Error::state_io_from)?;
            if metadata(&path, false)?.as_ref().map(Identity::of) != Some(live) {
                return Ok(Removed::Retained(Warning::IdentityChanged));
            }
            let removed = match fs::remove_file(&path) {
                Ok(()) => Removed::Deleted(file.bytes()),
                Err(error) if error.kind() == ErrorKind::NotFound => Removed::AlreadyAbsent,
                Err(error) => return Err(Error::state_io_from(error)),
            };
            Cleanup
                .observe(Step::AfterRemove)
                .map_err(Error::state_io_from)?;
            removed
        }
        None => Removed::AlreadyAbsent,
    };
    sync_directory(&parent)?;
    Ok(outcome)
}

#[cfg(test)]
mod tests;
