//! Explicit, bounded requests for declarations in the current summary.
use crate::{
    Error, Result,
    selection::Filter,
    values::{RelationPredicate, Slug, Timestamp},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod result;
pub use result::*;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, clap::ValueEnum,
)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Outgoing,
    Incoming,
    Both,
}

/// Exact anchors, explicit predicates and selection filters. No inferred relations.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Query {
    #[schemars(with = "String")]
    pub anchor: Slug,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub to: Option<Slug>,
    #[schemars(length(min = 1, max = 4))]
    pub predicates: Vec<RelationPredicate>,
    #[serde(default)]
    pub direction: Direction,
    #[serde(default = "default_depth")]
    #[schemars(range(min = 0, max = 4))]
    pub depth: usize,
    #[serde(default)]
    pub reviewed_only: bool,
    #[serde(default)]
    pub strict: bool,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub after: Option<Timestamp>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub before: Option<Timestamp>,
    #[serde(default = "default_nodes")]
    #[schemars(range(min = 1, max = 1000))]
    pub max_nodes: usize,
    #[serde(default = "default_edges")]
    #[schemars(range(min = 1, max = 5000))]
    pub max_edges: usize,
    #[serde(default = "default_limit")]
    #[schemars(range(min = 1, max = 200))]
    pub limit: usize,
    #[serde(default = "default_evidence")]
    #[schemars(range(min = 1, max = 262_144))]
    pub max_evidence_bytes: usize,
    #[serde(default = "default_deadline")]
    #[schemars(range(min = 1, max = 500))]
    pub deadline_ms: u64,
}
pub const fn default_depth() -> usize {
    2
}
pub const fn default_nodes() -> usize {
    200
}
pub const fn default_edges() -> usize {
    1000
}
pub const fn default_limit() -> usize {
    50
}
pub const fn default_evidence() -> usize {
    64 * 1024
}
pub const fn default_deadline() -> u64 {
    100
}

impl Query {
    pub fn new(anchor: Slug, predicates: Vec<RelationPredicate>) -> Self {
        Self {
            anchor,
            predicates,
            to: None,
            direction: Direction::default(),
            depth: default_depth(),
            reviewed_only: false,
            strict: false,
            kind: None,
            tag: None,
            prefix: None,
            after: None,
            before: None,
            max_nodes: default_nodes(),
            max_edges: default_edges(),
            limit: default_limit(),
            max_evidence_bytes: default_evidence(),
            deadline_ms: default_deadline(),
        }
    }
    pub fn filter(&self) -> Filter<'_> {
        Filter {
            kind: self.kind.as_deref(),
            tag: self.tag.as_deref(),
            prefix: self.prefix.as_deref(),
            after: self.after.as_ref(),
            before: self.before.as_ref(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.predicates.is_empty()
            || self.predicates.len() > 4
            || self.depth > 4
            || !(1..=1000).contains(&self.max_nodes)
            || !(1..=5000).contains(&self.max_edges)
            || !(1..=200).contains(&self.limit)
            || !(1..=262_144).contains(&self.max_evidence_bytes)
            || !(1..=500).contains(&self.deadline_ms)
        {
            return Err(Error::invalid_graph_query());
        }
        self.filter().validate()
    }
}
