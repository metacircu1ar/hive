//! Stable installed paths and bounded structured child commands.
use super::{Result, invalid, io};
mod response;
use hive_setup_support::files;
use serde::{Deserialize, de::DeserializeOwned};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
#[cfg(test)]
mod tests;
#[derive(Deserialize)]
pub(super) struct Identity {
    pub product: String,
    pub compiled_features: Vec<String>,
}
pub(super) fn call<T: DeserializeOwned>(
    binary: &Path,
    args: &[String],
    timeout: Duration,
    inputs: &[&str],
) -> Result<T> {
    let output = execute(binary, args, Some(timeout))?;
    response::decode(inputs, output)
}
pub(super) fn progress<T: DeserializeOwned>(
    binary: &Path,
    args: &[String],
    inputs: &[&str],
) -> Result<T> {
    let output = execute(binary, args, None)?;
    response::decode(inputs, output)
}
fn execute(
    binary: &Path,
    args: &[String],
    timeout: Option<Duration>,
) -> Result<hive_setup_support::runner::Output> {
    let path = binary
        .to_str()
        .ok_or_else(|| invalid("Executable path must be UTF-8."))?;
    let signals = hive_setup_support::runner::Signals::new().map_err(io)?;
    let mut command = Command::new(path);
    command.args(args);
    signals.configure_child(&mut command);
    let result =
        hive_setup_support::runner::owner_command(crate::os::spawn, &mut command, timeout, &|| {
            signals.cancelled()
        });
    signals.finish(result).map_err(io)
}
pub(super) fn invoked() -> Result<PathBuf> {
    let arg = std::env::args_os()
        .next()
        .ok_or_else(|| invalid("Cannot locate invoked executable."))?;
    files::executable(Path::new(&arg)).map_err(io)
}
pub(in crate::cli) fn skills(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        let selected = files::location(path).and_then(|resolved| {
            if path.as_os_str().is_empty()
                || resolved.to_str().is_none()
                || !std::fs::metadata(&resolved)?.is_dir()
            {
                return Err(hive_setup_support::invalid("Invalid skills directory."));
            }
            Ok(resolved)
        });
        return selected.map_err(|cause| {
            crate::cli::input::Context::new("setup", "--skills-dir")
                .selected_path(
                    cause,
                    "an existing accessible skills directory with a UTF-8 absolute path",
                )
                .error
        });
    }
    // A checkout build uses its source skills. The checkout is found at run time so
    // the build directory is not compiled into released binaries.
    if let Some(source) = crate::executable::path()?
        .ancestors()
        .find(|dir| dir.ends_with("target/release"))
        .and_then(|release| release.parent()?.parent())
        && source.join("Cargo.toml").is_file()
    {
        return Ok(source.join("skills"));
    }
    let binary = invoked()?;
    let prefix = binary
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| invalid("Cannot locate installed skills; pass --skills-dir."))?;
    Ok(prefix.join("share/hive/skills"))
}
pub(super) fn comb(explicit: Option<&Path>) -> Result<Option<PathBuf>> {
    if let Some(path) = explicit {
        return files::executable(path)
            .and_then(|resolved| {
                if resolved.to_str().is_none() {
                    return Err(hive_setup_support::invalid("Invalid executable path."));
                }
                Ok(Some(resolved))
            })
            .map_err(|cause| {
                crate::cli::input::Context::new("setup", "--comb-bin")
                    .selected_path(cause, "an existing executable with a UTF-8 absolute path")
                    .error
            });
    }
    let sibling = invoked()?.parent().map(|p| p.join("comb"));
    if let Some(path) = sibling
        && let Some(path) = files::optional_executable(&path).map_err(io)?
    {
        return Ok(Some(path));
    }
    files::optional_executable(Path::new("comb")).map_err(io)
}
