//! Runtime services and their public entry points.
// Shared crate-internal wire types are grouped behind one explicit facade.
pub(crate) mod api;
pub(crate) mod attachments;
mod auth_fallback;
pub use auth_fallback::Availability;
mod binaries;
pub(crate) mod branching;
mod caller;
pub(crate) mod client;
#[cfg(feature = "comb")]
pub(crate) mod collectors;
#[cfg(feature = "comb")]
mod comb;
mod control_frame;
mod core;
mod daemon;
pub(crate) mod domain;
pub(crate) mod features;
mod hub;
pub(crate) mod limits;
pub(crate) mod live;
use crate::locks;
mod models;
mod owned_thread;
mod owner;
mod preflight;
mod provider;
mod read_api;
mod reader;
mod scheduled;
mod scheduler;
mod steering_canary;
mod store;
#[cfg(test)]
mod test_support;
mod tool_registry;
pub(crate) use steering_canary::{
    Verification, validate_selection as validate_steering_selection, verify,
};
mod worker;

#[cfg(test)]
// Corpus fixtures fail immediately on setup or assertion errors.
mod protocol_corpus_support;

pub(crate) use binaries::{Binaries, ProviderSelection};
pub(crate) use core::Core;
pub(crate) use daemon::{daemon_configuration, logging_configuration, serve, validate_logging};
pub(crate) use store::check_existing_format;
pub use store::{CompactPreview, CompactResult, compact, compact_preview};

use client::invalid_request;
pub use client::{client, response, socket_path};
pub(crate) use daemon::{maintenance_lock, purge_lock};
use hub::Hub as Shared;
pub(super) use models::ModelDiscovery;
pub(crate) use models::models;
#[cfg(test)]
pub(crate) use test_support::test_directory;
