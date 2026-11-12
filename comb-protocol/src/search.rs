//! Validated retrieval request payloads.
use crate::{Error, Result, values::Timestamp};
use serde::{Deserialize, Serialize};
pub mod limits;
pub use crate::selection::{Filter, PageFilters, date_range, filter_text};
pub use limits::DEFAULT_SEARCH_LIMIT;
use limits::{QUERY_BYTES, SEARCH_LIMIT_MAX, SEARCH_LIMIT_MIN};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Search {
    pub query: String,
    pub limit: usize,
    pub kind: Option<String>,
    pub tag: Option<String>,
    pub prefix: Option<String>,
    pub after: Option<Timestamp>,
    pub before: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph: Option<crate::graph::Query>,
}
impl Default for Search {
    fn default() -> Self {
        Self {
            query: String::new(),
            limit: DEFAULT_SEARCH_LIMIT,
            kind: None,
            tag: None,
            prefix: None,
            after: None,
            before: None,
            graph: None,
        }
    }
}
impl Search {
    pub fn filter(&self) -> Filter<'_> {
        Filter {
            kind: self.kind.as_deref(),
            tag: self.tag.as_deref(),
            prefix: self.prefix.as_deref(),
            after: self.after.as_ref(),
            before: self.before.as_ref(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Text,
    Vector,
    #[default]
    Hybrid,
}

pub fn validate(request: &Search) -> Result<()> {
    query(&request.query)?;
    if !(SEARCH_LIMIT_MIN..=SEARCH_LIMIT_MAX).contains(&request.limit) {
        return Err(Error::invalid_search());
    }
    request.filter().validate()?;
    if let Some(graph) = &request.graph {
        graph.validate()?;
    }
    Ok(())
}

pub fn query(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > QUERY_BYTES {
        return Err(Error::invalid_query());
    }
    Ok(())
}
