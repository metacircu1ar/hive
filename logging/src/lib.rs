#![expect(
    clippy::disallowed_methods,
    reason = "Diagnostic sink and test writes are gated by the reviewed maintainer write \
        inventory"
)]
//! Bounded structured diagnostics with no raw error or provider text.
mod config;
mod counters;
mod fallback;
mod guard;
mod panic;
mod policy;
mod producer;
pub mod raw;
pub mod reader;
mod record;
mod sink;
mod stderr;
mod subscriber;
mod worker;

pub use config::Config;
use counters::Counters;
pub use counters::Counts;
use fallback::Spawn;
pub use guard::{Finished, Guard, Shutdown};
pub use subscriber::{Backend, InstallError, start, start_owned, start_stderr, start_writer};

use producer::Logger;
#[cfg(test)]
use producer::{Admission, Receipt};
pub use record::{Cause, Causes};
use record::{Fields, Invalid, Level};

/// Cap a native byte/count representation at `u64::MAX` without wrapping.
/// This is exact on the supported 32-bit and 64-bit pointer widths.
#[inline]
fn byte_count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
