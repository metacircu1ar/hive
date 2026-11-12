//! Current reference reports, separate from ordinary-only graph traversal.
use crate::{
    graph::Generation,
    values::{LinkKind, RelationPredicate, Slug},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Typed {
    pub predicate: RelationPredicate,
    pub unverified: bool,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Reference {
    pub slug: Slug,
    pub ordinary: Vec<LinkKind>,
    /// Ordinary links have source-page trust, not per-declaration review evidence.
    pub ordinary_source_page_unverified: Option<bool>,
    pub typed: Vec<Typed>,
    /// None means the neighboring page does not exist in this index generation.
    pub page_unverified: Option<bool>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    pub slug: Slug,
    pub incoming: bool,
    pub generation: Generation,
    pub references: Vec<Reference>,
    pub next_after: Option<Slug>,
    /// Extraction completeness is independent of pagination.
    pub complete: bool,
    pub current_incomplete_sources: u64,
}
