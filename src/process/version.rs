//! Capture native version output while callers retain their validation and error precedence.
use super::{CaptureOptions, Captured, CapturedOutput, Command, LauncherCache, Operation, Result};

pub(crate) fn version_probe<T>(
    launcher: &LauncherCache,
    command: &mut Command,
    operation: &mut Operation,
    options: CaptureOptions<'_>,
    inspect: impl FnOnce(CapturedOutput, &Operation) -> Result<T>,
) -> Result<T> {
    let mut child = Captured::start_configured(launcher, command, operation, options)?;
    let output = child.output()?;
    // Keep the captured pipes and receipt alive through caller validation.
    inspect(output, operation)
}

#[cfg(test)]
mod tests;
