//! Canonical index SQL. Transaction and statement ownership stay with callers.
pub(super) mod cache;
pub(super) mod chunks;
pub(super) mod embedding;
pub(super) mod embedding_write;
pub(super) mod links;
pub(super) mod pages;
pub(super) mod relations;
pub(super) mod schema;
pub(super) mod search;
pub(super) mod snapshot;

#[cfg(test)]
mod tests;

pub(super) mod graph;

pub(super) mod link_report;

pub(super) mod graph_diagnostics;
