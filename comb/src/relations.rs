//! Derived assertions over admitted page text, separate from ordinary links.
use crate::{
    Page,
    values::{EvidenceId, RelationPredicate, Slug, Zone},
};
use serde::{Deserialize, Serialize};

mod parse;
#[cfg(test)]
mod tests;

pub(crate) const OCCURRENCE_LIMIT: usize = 256;
pub(crate) const REFERENCE_LIMIT: usize = 64;
pub(crate) const REFERENCE_BYTES: usize = 512;
const DIAGNOSTIC_LIMIT: usize = 64;
pub(crate) const EXTRACTION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Occurrence {
    pub zone: Zone,
    pub entry: Option<EvidenceId>,
    pub start: usize,
    pub end: usize,
    pub predicate: RelationPredicate,
    pub target: Slug,
    pub verified: bool,
    pub references: Vec<String>,
    pub references_complete: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Issue {
    DuplicateSection,
    MalformedToken,
    UnknownPredicate,
    InvalidTarget,
    OccurrenceLimit,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Diagnostic {
    pub zone: Zone,
    pub entry: Option<EvidenceId>,
    pub start: usize,
    pub end: usize,
    pub issue: Issue,
}

#[derive(Default)]
pub(crate) struct Extraction {
    pub occurrences: Vec<Occurrence>,
    pub diagnostics: Vec<Diagnostic>,
    pub current_incomplete: bool,
    pub historical_incomplete: bool,
    pub diagnostics_omitted: usize,
}

impl Extraction {
    fn diagnostic(&mut self, diagnostic: Diagnostic) {
        match diagnostic.zone {
            Zone::Compiled => self.current_incomplete = true,
            Zone::Timeline => self.historical_incomplete = true,
        }
        if self.diagnostics.len() < DIAGNOSTIC_LIMIT {
            self.diagnostics.push(diagnostic);
        } else {
            self.diagnostics_omitted += 1;
        }
    }
}

pub(crate) fn extract(page: &Page) -> Extraction {
    let mut result = Extraction::default();
    parse::append(
        &mut result,
        &page.compiled,
        None,
        page.verified(),
        OCCURRENCE_LIMIT,
    );
    let reviewed = page.reviewed_start();
    let mut historical = 0;
    for (position, entry) in page.timeline.iter().enumerate() {
        let before = result.occurrences.len();
        parse::append(
            &mut result,
            &entry.text,
            Some(&entry.metadata.authored.id),
            entry.metadata.owned().actor().is_human()
                || reviewed.is_some_and(|start| position >= start),
            OCCURRENCE_LIMIT - historical,
        );
        historical += result.occurrences.len() - before;
    }
    result
}
