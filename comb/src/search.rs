//! Filtered page-level results and query vectors.
pub(crate) mod pages;
use crate::{
    Error, ErrorCode, Result,
    embedding::{Coverage, Query},
    values::{CreateSafety, MatchEvidence, Route, Slug, Zone},
};
pub use comb_protocol::search::{DEFAULT_SEARCH_LIMIT, Filter, Mode, Search, limits, validate};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hit {
    pub slug: Slug,
    pub title: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub score: f64,
    pub matched_by: Route,
    pub snippet: String,
    /// UTF-8 byte ranges for literal query words within the snippet.
    pub highlights: Vec<[usize; 2]>,
    pub heading: String,
    pub zone: Zone,
    pub evidence: MatchEvidence,
    pub create_safety: CreateSafety,
    pub unverified: bool,
    pub page_unverified: bool,
}
#[derive(Serialize, Deserialize)]
pub struct Results {
    pub results: Vec<Hit>,
    pub mode: Mode,
    pub fallback: Option<ErrorCode>,
    pub coverage: Option<Coverage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<Box<crate::graph::Report>>,
}

pub enum QueryVector {
    NotRequested,
    Vector(Query),
    Unavailable(Error),
}
impl From<Result<Query>> for QueryVector {
    fn from(query: Result<Query>) -> Self {
        match query {
            Ok(query) => Self::Vector(query),
            Err(error) => Self::Unavailable(error),
        }
    }
}

impl QueryVector {
    pub(crate) fn into_result(self) -> Result<Query> {
        match self {
            Self::NotRequested => Err(Error::query_model_missing()),
            Self::Vector(query) => Ok(query),
            Self::Unavailable(error) => Err(error),
        }
    }
}
