//! Use-time checks of a server binary. There is no version pin or allowlist: a
//! version this build has not tested is a warning, never a refusal.
use super::{BUNDLED, Binary, Device, Pin};
use crate::{Error, Result, embedding::HttpConfig};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const VERSION_TIMEOUT: Duration = Duration::from_secs(30);

/// A binary that passed its use-time checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub program: PathBuf,
    pub device: Device,
    /// Whether its reported build commit is the pinned, tested one.
    pub tested: bool,
}

/// Step one of a selection: the binary exists, is executable and runs `--version`.
pub fn check_binary(binary: &Binary, pin: &Pin, cancelled: &dyn Fn() -> bool) -> Result<Checked> {
    let (program, device) = resolve(binary)?;
    let tested = version(&program, pin, cancelled)?;
    if !tested {
        tracing::warn!(name: "comb.llama_untested_version", decision = "warn_only");
    }
    Ok(Checked {
        program,
        device,
        tested,
    })
}

/// The executable a selection runs and the device its argv targets.
pub(crate) fn resolve(binary: &Binary) -> Result<(PathBuf, Device)> {
    match binary {
        Binary::Bundled => Ok((bundled()?, Device::bundled())),
        Binary::Path(path) => Ok((user(path)?, Device::Own)),
    }
}

/// The bundled server beside the running `comb`, in the same installed generation.
pub fn bundled() -> Result<PathBuf> {
    let current = std::env::current_exe()?.canonicalize()?;
    let path = current
        .parent()
        .ok_or_else(Error::llama_server_missing)?
        .join(BUNDLED);
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 => {
            Ok(path)
        }
        Ok(_) => Err(Error::llama_server_missing()),
        Err(error) => Err(Error::llama_server_missing().caused_by(error)),
    }
}

fn user(path: &Path) -> Result<PathBuf> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::llama_server_not_found().caused_by(error));
        }
        Err(error) => return Err(Error::llama_server_not_executable().caused_by(error)),
    };
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(Error::llama_server_not_executable());
    }
    Ok(std::path::absolute(path)?)
}

/// Run `--version`; true when the reported commit is the pinned one.
pub(crate) fn version(program: &Path, pin: &Pin, cancelled: &dyn Fn() -> bool) -> Result<bool> {
    let mut command = Command::new(program);
    command.arg("--version");
    let output = hive_setup_support::runner::capture_command(
        Command::spawn,
        &mut command,
        VERSION_TIMEOUT,
        cancelled,
    )
    .map_err(|cause| Error::llama_server_version_failed().caused_by(cause))?;
    if output.code != 0 {
        return Err(Error::llama_server_version_failed());
    }
    Ok(reported_commit(&output.stderr)
        .or_else(|| reported_commit(&output.stdout))
        .is_some_and(|commit| pin.tested(commit)))
}

/// The commit in llama.cpp's `version: VERSION (build N, commit COMMIT)` line.
pub(crate) fn reported_commit(text: &str) -> Option<&str> {
    text.lines().find_map(|line| {
        let (version, details) = line
            .trim()
            .strip_prefix("version: ")?
            .split_once(" (build ")?;
        let (number, commit) = details.strip_suffix(')')?.split_once(", commit ")?;
        (!version.is_empty()
            && version.len() <= 96
            && version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
            && !number.is_empty()
            && number.bytes().all(|b| b.is_ascii_digit())
            && (7..=40).contains(&commit.len())
            && commit
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        .then_some(commit)
    })
}

/// Step four: the ready server returns normalized 4,096-dimensional embeddings.
pub(crate) fn embeddings(config: &HttpConfig) -> Result<()> {
    crate::provision::validate_http(config.clone())
        .map_err(|cause| Error::llama_server_probe_failed().caused_by(cause))
}
