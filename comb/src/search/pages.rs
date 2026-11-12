//! Shared page read results, independent of index and writer ownership.
use crate::{
    Page,
    values::{ContentHash, FindingKind, Slug},
};
use serde::{Deserialize, Serialize};

pub(crate) const FINDING_LIMIT: u16 = 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    pub slug: Slug,
    pub hash: ContentHash,
    pub page: Page,
    pub markdown: String,
    pub unverified: bool,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Summary {
    pub slug: Slug,
    pub title: String,
    pub kind: String,
    pub hash: ContentHash,
    pub unverified: bool,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Stats {
    pub embedding_runtime: Option<crate::embedding::RuntimeInfo>,
    pub pages: u64,
    pub chunks: u64,
    pub links: u64,
    pub inbox: u64,
    pub unverified: u64,
    pub unresolved_proposals: u64,
    pub quarantined_pages: u64,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Finding {
    pub slug: String,
    pub kind: FindingKind,
    pub detail: String,
}
