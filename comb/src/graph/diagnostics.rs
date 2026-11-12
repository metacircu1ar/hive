//! Bounded structural observations; these do not establish evidence support.
use super::{ExtractionIssue, Generation};
use crate::values::{ContentHash, EvidenceId, RelationPredicate, Slug, Zone};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Location {
    pub source: Slug,
    pub source_hash: ContentHash,
    pub zone: Zone,
    pub entry: Option<EvidenceId>,
    pub start: usize,
    pub end: usize,
    pub page_unverified: bool,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct PastOccurrence {
    pub location: Location,
    pub predicate: RelationPredicate,
    pub target: Slug,
    pub unverified: bool,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemKind {
    Extraction,
    MissingTarget,
    UnresolvedEvidence,
    SupportLimit,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Problem {
    pub location: Location,
    pub kind: ProblemKind,
    pub extraction_issue: Option<ExtractionIssue>,
    pub reference: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Diagnostics {
    pub generation: Generation,
    pub current_reviewed: u64,
    pub current_unreviewed: u64,
    pub historical_reviewed: u64,
    pub historical_unreviewed: u64,
    pub current_incomplete_sources: u64,
    pub historical_incomplete_sources: u64,
    pub problems: Vec<Problem>,
    pub problems_omitted: u64,
    pub past_occurrences: Vec<PastOccurrence>,
    pub past_occurrences_omitted: u64,
}
impl Diagnostics {
    pub(crate) fn problem(&mut self, problem: Problem) {
        if self.problems.len() < 256 {
            self.problems.push(problem);
        } else {
            self.problems_omitted += 1;
        }
    }
    pub(crate) fn past(&mut self, occurrence: PastOccurrence) {
        if self.past_occurrences.len() < 256 {
            self.past_occurrences.push(occurrence);
        } else {
            self.past_occurrences_omitted += 1;
        }
    }
}
