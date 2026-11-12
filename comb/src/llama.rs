//! Comb's managed llama-server for local embeddings.
//!
//! One pinned model lives in Comb's private cache. The bundled `comb-llama-server`
//! next to `comb`, or a user-chosen `llama-server`, serves it on loopback with one
//! validated argv. Comb owns that child: it waits for readiness, never restarts it
//! silently, and stops and reaps it. If the server fails, a serving Comb keeps text
//! search and reports embeddings unavailable ([`Availability`]). The stored
//! selection is the ordinary HTTP embedding configuration plus one
//! [`ManagedServer`] marker.
mod argv;
mod download;
mod memory;
mod pin;
mod probe;
mod process;
mod selection;
#[cfg(test)]
mod tests;

pub use argv::{Device, argv, config};
pub use download::{Progress, Source};
pub use memory::{admit as admit_memory, available as available_memory};
pub use pin::{Model, Pin};
pub use probe::{Checked, bundled, check_binary};
pub use process::{Availability, Launch, Running};
pub(crate) use selection::preflight;
pub use selection::{
    Binary, ManagedServer, Prepared, cache_directory, choose_port, prepare, with_server,
};
pub(crate) use selection::{check_arguments, previous_port};
use std::time::Duration;

/// A caller's human progress display for a model download; never logged.
pub type Reporter = std::sync::Arc<dyn Fn(Progress) + Send + Sync>;

/// The built-in selection name accepted by `comb model use`.
pub const CHOICE: &str = "qwen3-local";
/// The bundled server's file name, installed next to `comb`.
pub const BUNDLED: &str = "comb-llama-server";
/// Comb's private model cache, below `$XDG_CACHE_HOME` or `~/.cache`.
pub const MODEL_CACHE: &str = "comb/models";
/// Loopback only. The embedding space identity includes the URL and its port.
pub const HOST: &str = "127.0.0.1";
/// The port a new selection prefers, matching the documented example configuration.
pub const PREFERRED_PORT: u16 = 8081;
pub const ALIAS: &str = "qwen3-embedding-8b";
pub const DIMENSIONS: usize = 4096;
/// Parallel server slots; each slot holds one request input.
pub const SLOTS: usize = 4;
/// Metal and user-chosen servers keep the validated 8,192-token slots.
pub const SLOT_TOKENS: usize = 8192;
pub const TIMEOUT_SECONDS: u64 = 60;
/// The bundled CPU server bounds one input to fewer tokens so that it fits Comb's
/// 120-second embedding deadline; longer inputs are split, never truncated.
/// Provisional until measured on the documented Linux floor.
pub const CPU_SLOT_TOKENS: usize = 2048;
/// Conservative whole-request UTF-8 budget for portable CPU inference.
pub const CPU_REQUEST_BYTES: usize = 512;
pub const CPU_TIMEOUT_SECONDS: u64 = 120;
/// Conservative admission floor: physical memory on macOS; available memory,
/// after use and cgroup limits, on Linux. It is not a 16 GiB Linux host minimum.
pub const MEMORY_FLOOR_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// Loading 8 GB of weights from a cold disk, including CPU hosts.
pub const READY_DEADLINE: Duration = Duration::from_secs(600);
/// SIGTERM grace before SIGKILL; within the login service's stop budget.
pub const STOP_GRACE: Duration = Duration::from_secs(10);
/// Bounded, private tail of the server's stderr, attached only to failure causes.
pub const STDERR_TAIL_BYTES: usize = 64 * 1024;
/// Free space kept beyond the remaining model bytes during a download.
pub const DOWNLOAD_HEADROOM_BYTES: u64 = 512 * 1024 * 1024;
/// A download that receives nothing for this long fails and can be resumed.
pub const DOWNLOAD_STALL: Duration = Duration::from_secs(60);
/// Qwen3-Embedding's retrieval instruction; documents have no prefix.
pub const QUERY_PREFIX: &str = "Instruct: Given a question about our project, retrieve the \
    knowledge pages that answer it\nQuery: ";
