//! Returned declarations are assertions with provenance, never independent proof.
use crate::values::{ContentHash, EvidenceId, RelationPredicate, Slug, Zone};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Generation {
    pub identity: String,
    pub revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub slug: Slug,
    pub content_hash: ContentHash,
    pub page_unverified: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Span {
    pub zone: Zone,
    pub entry: Option<EvidenceId>,
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub unverified: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Support {
    pub entry: EvidenceId,
    pub text: String,
    pub unverified: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edge {
    pub source: Slug,
    pub predicate: RelationPredicate,
    pub target: Slug,
    pub source_hash: ContentHash,
    pub page_unverified: bool,
    pub declaration: Span,
    pub support: Vec<Support>,
    pub unresolved_references: Vec<String>,
    /// Known support references omitted because the shared evidence budget ran out.
    pub omitted_references: usize,
    pub support_complete: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Path {
    pub nodes: Vec<Slug>,
    pub edges: Vec<Edge>,
    pub unverified: bool,
    /// True only for a complete bounded search over the selected index scope.
    pub shortest: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Coverage {
    /// None when cancellation/deadline prevented checking the incomplete-source index.
    pub current_incomplete_sources: Option<u64>,
    /// Optional accounting collected after traversal, within the remaining deadline.
    pub totals: Option<CoverageTotals>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoverageTotals {
    pub pages: u64,
    pub current_occurrences: u64,
    pub historical_occurrences: u64,
    pub historical_incomplete_sources: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Incomplete {
    Extraction,
    Nodes,
    Edges,
    Matches,
    EvidenceBytes,
    Deadline,
    Cancelled,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Work {
    pub nodes_visited: usize,
    pub edges_examined: usize,
    pub evidence_bytes: usize,
    pub unresolved_targets: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub request: super::Query,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_filter: Option<crate::selection::PageFilters>,
    pub generation: Generation,
    pub nodes: Vec<Node>,
    pub relations: Vec<Edge>,
    pub connection: Option<Path>,
    pub coverage: Coverage,
    pub work: Work,
    pub complete: bool,
    pub incomplete_reasons: Vec<Incomplete>,
}
