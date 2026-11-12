//! Auth-free native execution diagnostics, independent of configured policy and login.
use crate::config::{Config, Provider};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) const TIMEOUT: Duration = Duration::from_secs(5);
const MARKER: &str = "hive-native-sandbox-ready";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Passed,
    Failed,
    Unavailable,
    Busy,
    Unsupported,
    NotConfigured,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Report {
    pub outcome: Outcome,
    /// Display only; execution and provider identity retain the native path bytes.
    #[serde(deserialize_with = "Option::deserialize")]
    pub executable: Option<String>,
    pub code: String,
}

impl Report {
    pub(crate) fn new(outcome: Outcome, executable: Option<PathBuf>, code: &str) -> Self {
        Self {
            outcome,
            executable: executable.map(|path| path.to_string_lossy().into_owned()),
            code: code.into(),
        }
    }
}

pub(crate) fn configured(config: &Config) -> bool {
    configured_for(config, Provider::Codex)
}

pub(crate) fn configured_for(config: &Config, provider: Provider) -> bool {
    config
        .auth_profiles
        .values()
        .any(|profile| profile.provider == provider)
        || config.providers.contains_key(&provider)
}

pub(crate) fn run(binary: &Path) -> Report {
    run_for(binary, Provider::Codex)
}

pub(crate) fn run_for(binary: &Path, provider: Provider) -> Report {
    if !cfg!(target_os = "linux") {
        return Report::new(Outcome::Unsupported, Some(binary.into()), "linux_only");
    }
    match execute_for(binary, provider, TIMEOUT) {
        Ok(report) => report,
        Err(error) => failed(binary, &error),
    }
}

fn failed(binary: &Path, error: &std::io::Error) -> Report {
    let code = if error.kind() == std::io::ErrorKind::TimedOut {
        "timed_out"
    } else {
        "probe_io_failed"
    };
    tracing::warn!(name: "sandbox.probe_io_failed",
         code = code,
         error = error as &(dyn std::error::Error + 'static),
         decision = "return_failed_diagnostic");
    Report::new(Outcome::Failed, Some(binary.into()), code)
}

#[cfg(test)]
fn execute(binary: &Path, timeout: Duration) -> std::io::Result<Report> {
    execute_for(binary, Provider::Codex, timeout)
}

fn execute_for(binary: &Path, provider: Provider, timeout: Duration) -> std::io::Result<Report> {
    // Neither native user configuration nor authentication is copied into this home.
    let directory = probe_directory()?;
    let result = match provider {
        Provider::Codex => execute_in(binary, directory.path(), timeout),
        Provider::Claude => claude::execute_in(binary, directory.path(), timeout),
    };
    finish_probe(result, directory.close())
}

fn probe_directory() -> std::io::Result<tempfile::TempDir> {
    // Codex refuses helper aliases below system temp; older Bubblewrap needs them.
    // Reuse the existing owner-only non-temp root, with a fresh empty home per probe.
    let root = crate::launch::system_cache()
        .protected_root()
        .map_err(std::io::Error::other)?;
    tempfile::Builder::new()
        .prefix(".sandbox-probe-")
        .permissions(std::fs::Permissions::from_mode(
            crate::os::PRIVATE_DIRECTORY_MODE,
        ))
        .tempdir_in(root)
}

fn finish_probe(
    result: std::io::Result<Report>,
    cleanup: std::io::Result<()>,
) -> std::io::Result<Report> {
    match (result, cleanup) {
        (mut result, Err(error)) => {
            let decision = match &mut result {
                Ok(report) if report.outcome == Outcome::Passed => {
                    report.outcome = Outcome::Failed;
                    report.code = "probe_cleanup_failed".into();
                    "return_failed_diagnostic"
                }
                _ => "preserve_probe_failure",
            };
            tracing::warn!(name: "sandbox.probe_cleanup_failed",
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = decision);
            result
        }
        (result, Ok(())) => result,
    }
}

fn execute_in(binary: &Path, home: &Path, timeout: Duration) -> std::io::Result<Report> {
    // A shell resolves its working directory physically (for example /var to
    // /private/var on macOS); use that same path for HOME and CODEX_HOME.
    let home = home.canonicalize()?;
    let mut command = crate::process::command(binary, Provider::Codex, &home, &home);
    command.args([
        "-c",
        "sandbox_mode=\"read-only\"",
        "sandbox",
        "--",
        "/bin/echo",
        MARKER,
    ]);
    let output = hive_setup_support::runner::capture_command(
        crate::os::spawn,
        &mut command,
        timeout,
        &|| false,
    )?;
    Ok(classify(binary, &output))
}

fn classify(binary: &Path, output: &hive_setup_support::runner::Output) -> Report {
    let (outcome, code) = if output.code == 0 && output.stdout.trim() == MARKER {
        (Outcome::Passed, "native_command_completed")
    } else if output.stderr.contains("Operation not permitted")
        || output.stderr.contains("Permission denied")
    {
        (Outcome::Failed, "sandbox_permission_denied")
    } else {
        (Outcome::Failed, "native_command_failed")
    };
    if outcome == Outcome::Failed {
        tracing::warn!(name: "sandbox.native_command_failed",
             code = code,
             exit_code = output.code,
             decision = "return_failed_diagnostic");
    }
    // Native output can contain arbitrary provider text. Publish only this bounded classification.
    Report::new(outcome, Some(binary.into()), code)
}

mod claude;

#[cfg(test)]
mod tests;
