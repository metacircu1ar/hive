//! Resumable, verified model download into Comb's private cache. Bytes stream to
//! disk and are never buffered whole; a cached file is rehashed before reuse.
use super::{DOWNLOAD_HEADROOM_BYTES, DOWNLOAD_STALL, Pin};
use crate::{Error, ErrorCode, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

const CHUNK: usize = 1024 * 1024;
const CONNECT: Duration = Duration::from_secs(30);
const REDIRECTS: usize = 10;

/// One pinned file: where it comes from and the content it must have.
#[derive(Clone, Copy, Debug)]
pub struct Source<'a> {
    pub url: &'a str,
    pub file: &'a str,
    pub sha256: &'a str,
    pub bytes: u64,
}
impl<'a> Source<'a> {
    pub fn pinned(pin: &'a Pin, url: &'a str) -> Self {
        Self {
            url,
            file: &pin.model.file,
            sha256: &pin.model.sha256,
            bytes: pin.model.bytes,
        }
    }
}

/// Download events for a human progress line; never logged with values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// A transfer starts or resumes at `offset` of `total` bytes.
    Started {
        offset: u64,
        total: u64,
    },
    Received {
        offset: u64,
        total: u64,
    },
    /// The complete file is being hashed.
    Verifying,
}

/// The verified pinned file in `cache`: reused when its rehash matches, otherwise
/// downloaded, resuming any partial file. Cancellation leaves the partial file.
pub fn ensure(
    cache: &Path,
    source: &Source<'_>,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf> {
    crate::provision::files::directory(cache)?;
    let _lock = hive_setup_support::locks::path(cache, "comb-model", true).map_err(|cause| {
        if cause.kind() == io::ErrorKind::WouldBlock {
            Error::model_download_busy().caused_by(cause)
        } else {
            Error::from(cause)
        }
    })?;
    let target = cache.join(source.file);
    if cached(&target, source, cancelled, progress)? {
        return Ok(target);
    }
    let partial = cache.join(format!("{}.part", source.file));
    let mut file = open_partial(&partial)?;
    let mut offset = file.metadata()?.len();
    if offset > source.bytes {
        drop(file);
        remove(&partial)?;
        file = open_partial(&partial)?;
        offset = 0;
    }
    if offset < source.bytes {
        free_space(cache, source.bytes - offset)?;
        offset = match transfer(&mut file, source, offset, cancelled, progress) {
            Err(error) if error.code == ErrorCode::InvalidEmbeddingAsset => {
                drop(file);
                return Err(crate::service::auth::with_cleanup(error, remove(&partial)));
            }
            result => result?,
        };
    }
    file.sync_all()?;
    drop(file);
    if offset != source.bytes {
        // A short transfer keeps its bytes; the next attempt resumes there.
        return Err(Error::model_download_failed());
    }
    progress(Progress::Verifying);
    if let Err(error) = verify(&partial, source, cancelled) {
        if error.code == ErrorCode::InvalidEmbeddingAsset {
            return Err(crate::service::auth::with_cleanup(error, remove(&partial)));
        }
        return Err(error);
    }
    hive_setup_support::removal::rename_noreplace(&partial, &target)?;
    File::open(cache)?.sync_all()?;
    tracing::info!(name: "comb.model_downloaded", bytes = source.bytes);
    Ok(target)
}

fn cached(
    target: &Path,
    source: &Source<'_>,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(Progress),
) -> Result<bool> {
    if hive_setup_support::files::optional_metadata(target)?.is_some() {
        progress(Progress::Verifying);
        match verify(target, source, cancelled) {
            Ok(()) => return Ok(true),
            Err(error) if error.code == ErrorCode::InvalidEmbeddingAsset => {
                if let Err(cleanup) = remove(target) {
                    return Err(crate::service::auth::with_cleanup(error, Err(cleanup)));
                }
                tracing::warn!(name: "comb.model_cache_discarded", decision = "download_again");
            }
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

/// A selected model that still exists as an owned regular file of the pinned size.
/// Serve checks this on every start; `ensure` rehashes on every selection.
pub(crate) fn present(path: &Path, bytes: u64) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(Error::model_missing().caused_by(error));
        }
        Err(error) => return Err(error.into()),
    };
    if !owned_file(&metadata) || metadata.len() != bytes {
        return Err(Error::model_missing());
    }
    Ok(())
}

fn owned_file(metadata: &fs::Metadata) -> bool {
    metadata.is_file()
        && metadata.nlink() == 1
        && metadata.uid() == rustix::process::geteuid().as_raw()
        && metadata.mode() & 0o022 == 0
}

fn open_partial(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    if !owned_file(&file.metadata()?) {
        return Err(Error::invalid_embedding_asset());
    }
    Ok(file)
}

fn free_space(cache: &Path, remaining: u64) -> Result<()> {
    let stat = rustix::fs::statvfs(cache).map_err(io::Error::from)?;
    let free = stat.f_bavail.saturating_mul(stat.f_frsize);
    let needed = remaining.saturating_add(DOWNLOAD_HEADROOM_BYTES);
    if free < needed {
        return Err(Error::model_disk_space(needed - free));
    }
    Ok(())
}

fn client() -> Result<reqwest::blocking::Client> {
    // The blocking client's timeout bounds each body read: a stall, not the transfer.
    reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT)
        .timeout(DOWNLOAD_STALL)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= REDIRECTS {
                attempt.error("too many model download redirects")
            } else if attempt.url().scheme() != "https" {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|cause| Error::model_download_failed().caused_by(cause))
}

fn transfer(
    file: &mut File,
    source: &Source<'_>,
    offset: u64,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(Progress),
) -> Result<u64> {
    let (mut response, mut offset) = open_response(file, source, offset)?;
    file.seek(SeekFrom::Start(offset))?;
    progress(Progress::Started {
        offset,
        total: source.bytes,
    });
    let mut buffer = vec![0; CHUNK];
    loop {
        if cancelled() {
            return Err(io::Error::from(io::ErrorKind::Interrupted).into());
        }
        let count = match response.read(&mut buffer) {
            Ok(0) => return Ok(offset),
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(Error::model_download_failed().caused_by(error)),
        };
        let next = offset + crate::convert::usize_u64(count);
        if next > source.bytes {
            return Err(Error::model_size_mismatch());
        }
        file.write_all(&buffer[..count])?;
        offset = next;
        progress(Progress::Received {
            offset,
            total: source.bytes,
        });
    }
}

fn open_response(
    file: &mut File,
    source: &Source<'_>,
    mut offset: u64,
) -> Result<(reqwest::blocking::Response, u64)> {
    let mut request = client()?.get(source.url);
    if offset > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
    }
    let response = request
        .send()
        .map_err(|cause| Error::model_download_failed().caused_by(cause))?;
    match response.status().as_u16() {
        206 if offset > 0 => {
            let start = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| content_range(value, source.bytes));
            if start != Some(offset) {
                return Err(Error::model_download_failed());
            }
        }
        200 => {
            if offset > 0 {
                // The server ignored the range; restart this partial file from zero.
                tracing::info!(name: "comb.model_download_restarted", decision = "range_ignored");
                file.set_len(0)?;
                offset = 0;
            }
        }
        status => return Err(Error::model_download_status(status)),
    }
    if response
        .content_length()
        .is_some_and(|length| length != source.bytes - offset)
    {
        return Err(Error::model_size_mismatch());
    }
    Ok((response, offset))
}

/// The start of `bytes START-END/TOTAL` when it ends at the pinned size.
fn content_range(value: &str, total: u64) -> Option<u64> {
    let (range, size) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let (start, end, size) = (
        start.parse::<u64>().ok()?,
        end.parse::<u64>().ok()?,
        size.parse::<u64>().ok()?,
    );
    (size == total && end.checked_add(1) == Some(total) && start <= end).then_some(start)
}

/// An owned regular file of the pinned size and SHA-256; the name must not change.
fn verify(path: &Path, source: &Source<'_>, cancelled: &dyn Fn() -> bool) -> Result<()> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)?;
    let before = file.metadata()?;
    if !owned_file(&before) || before.len() != source.bytes {
        return Err(Error::model_size_mismatch());
    }
    let mut hash = Sha256::new();
    let mut buffer = vec![0; CHUNK];
    let mut total = 0_u64;
    loop {
        if cancelled() {
            return Err(io::Error::from(io::ErrorKind::Interrupted).into());
        }
        let count = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        hash.update(&buffer[..count]);
        total += crate::convert::usize_u64(count);
    }
    let named = fs::symlink_metadata(path)?;
    if total != source.bytes
        || (named.dev(), named.ino()) != (before.dev(), before.ino())
        || named.len() != before.len()
    {
        return Err(Error::model_size_mismatch());
    }
    if format!("{:x}", hash.finalize()) != source.sha256 {
        return Err(Error::model_checksum_mismatch());
    }
    Ok(())
}

fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(hive_setup_support::files::sync_parent(path)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
