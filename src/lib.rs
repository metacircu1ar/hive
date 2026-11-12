#![expect(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact reviewed maintainer write inventory."
)]
//! `Hive` command, provider and runtime library entry points.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("Hive supports only Linux and macOS.");

#[cfg(test)]
extern crate self as hive;

mod api;
mod approvals;
mod auth;
mod binary;
mod calls;
mod cli;
pub mod compatibility;
pub mod config;
mod convert;
mod credentials;
mod decode;
mod enums;
mod environment;
pub mod error;
mod executable;
mod external_mcp;
mod launch;
mod limits;
mod locks;
mod mcp;
mod native_sandbox;
mod observability;
pub mod options;
mod os;
mod process;
mod provider;
mod provider_support;
mod publication;
pub mod runtime;
mod shell;
mod sqlite;
pub mod state;

/// Dispatch trusted process modes, then run the CLI or standalone daemon.
pub fn run(daemon: bool) -> std::process::ExitCode {
    executable::capture();
    if let Some(status) = launch::dispatch() {
        return status;
    }
    if let Some(status) = shell::dispatch() {
        return status;
    }
    // Owned process groups are proven absent by this process; their orphans
    // must come back here to be reaped, never wait on a container's PID 1.
    #[cfg(target_os = "linux")]
    os::adopt_orphans();
    cli::run(daemon)
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod integration_support;

#[cfg(test)]
#[path = "../tests/support/reference.rs"]
mod reference;
