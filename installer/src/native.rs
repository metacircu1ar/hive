//! Installer-owned manager and maintenance groups share one scoped signal policy.
use crate::{Result, cancellation::Cancellation};
use hive_setup_support::runner::{self, Output};
use std::{process::Command, time::Duration};
pub(crate) fn capture(program: &str, args: &[String], timeout: Duration) -> Result<Output> {
    execute(program, args, Some(timeout))
}
pub(crate) fn progress(program: &str, args: &[String]) -> Result<Output> {
    execute(program, args, None)
}
fn execute(program: &str, args: &[String], timeout: Option<Duration>) -> Result<Output> {
    let cancellation = Cancellation::new()?;
    let mut command = Command::new(program);
    command.args(args);
    cancellation.configure_child(&mut command);
    let cancelled = || cancellation.cancelled();
    let result = runner::owner_command(Command::spawn, &mut command, timeout, &cancelled);
    cancellation.finish(result.map_err(Into::into))
}

#[cfg(test)]
mod tests;
