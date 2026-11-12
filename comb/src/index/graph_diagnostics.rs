//! Stream all diagnostic work while bounding retained details independently.
use super::queries::graph_diagnostics as sql;
use super::{Index, pages::count};
use crate::{
    Error, Result,
    graph::{Diagnostics, Generation, Location, PastOccurrence, Problem, ProblemKind},
    values::{ContentHash, EvidenceId, RelationPredicate, Slug, Zone},
};
use std::collections::BTreeSet;

impl Index {
    pub(crate) fn graph_diagnostics(&self) -> Result<Diagnostics> {
        self.graph_coverage_integrity()?;
        let generation = self.connection.query_row(sql::GENERATION, [], |r| {
            Ok(Generation {
                identity: r.get(0)?,
                revision: count(r, 1)?,
            })
        })?;
        let mut report = Diagnostics {
            generation,
            current_reviewed: 0,
            current_unreviewed: 0,
            historical_reviewed: 0,
            historical_unreviewed: 0,
            current_incomplete_sources: 0,
            historical_incomplete_sources: 0,
            problems: Vec::new(),
            problems_omitted: 0,
            past_occurrences: Vec::new(),
            past_occurrences_omitted: 0,
        };
        self.extraction_diagnostics(&mut report)?;
        self.occurrence_diagnostics(&mut report)?;
        Ok(report)
    }
    fn extraction_diagnostics(&self, report: &mut Diagnostics) -> Result<()> {
        let mut statement = self.connection.prepare(sql::COVERAGE)?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let source: Slug = row.get(0)?;
            let hash: ContentHash = row.get(1)?;
            let unverified = !row.get::<_, bool>(2)?;
            report.current_incomplete_sources += u64::from(row.get::<_, bool>(3)?);
            report.historical_incomplete_sources += u64::from(row.get::<_, bool>(4)?);
            report.problems_omitted += count(row, 6)?;
            let diagnostics: Vec<crate::relations::Diagnostic> =
                serde_json::from_str(&row.get::<_, String>(5)?)
                    .map_err(|cause| Error::graph_index_invalid().caused_by(cause))?;
            for d in diagnostics {
                report.problem(Problem {
                    location: Location {
                        source: source.clone(),
                        source_hash: hash.clone(),
                        zone: d.zone,
                        entry: d.entry,
                        start: d.start,
                        end: d.end,
                        page_unverified: unverified,
                    },
                    kind: ProblemKind::Extraction,
                    extraction_issue: Some(d.issue),
                    reference: None,
                });
            }
        }
        Ok(())
    }
    fn occurrence_diagnostics(&self, report: &mut Diagnostics) -> Result<()> {
        let mut statement = self.connection.prepare(sql::OCCURRENCES)?;
        let mut rows = statement.query([])?;
        let mut evidence: Option<(Slug, BTreeSet<String>)> = None;
        while let Some(row) = rows.next()? {
            let occurrence = Occurrence::decode(row)?;
            let past = occurrence.location.zone == Zone::Timeline;
            match (past, occurrence.verified) {
                (false, true) => report.current_reviewed += 1,
                (false, false) => report.current_unreviewed += 1,
                (true, true) => report.historical_reviewed += 1,
                (true, false) => report.historical_unreviewed += 1,
            }
            if past {
                report.past(PastOccurrence {
                    location: occurrence.location.clone(),
                    predicate: occurrence.predicate,
                    target: occurrence.target.clone(),
                    unverified: !occurrence.verified,
                });
            }
            if occurrence.missing {
                report.problem(occurrence.problem(
                    ProblemKind::MissingTarget,
                    Some(occurrence.target.to_string()),
                ));
            }
            if !occurrence.references_complete {
                report.problem(occurrence.problem(ProblemKind::SupportLimit, None));
            }
            if occurrence.references.is_empty() {
                continue;
            }
            if evidence
                .as_ref()
                .is_none_or(|(source, _)| source != &occurrence.location.source)
            {
                let page = self.document(&occurrence.location.source)?.page;
                evidence = Some((
                    occurrence.location.source.clone(),
                    page.timeline
                        .into_iter()
                        .map(|e| e.metadata.authored.id.to_string())
                        .collect(),
                ));
            }
            let (_, ids) = evidence.as_ref().ok_or_else(Error::graph_index_invalid)?;
            for reference in &occurrence.references {
                if !ids.contains(reference) {
                    report.problem(
                        occurrence
                            .problem(ProblemKind::UnresolvedEvidence, Some(reference.clone())),
                    );
                }
            }
        }
        Ok(())
    }
}

struct Occurrence {
    location: Location,
    predicate: RelationPredicate,
    target: Slug,
    verified: bool,
    references: Vec<String>,
    references_complete: bool,
    missing: bool,
}
impl Occurrence {
    fn decode(r: &rusqlite::Row<'_>) -> Result<Self> {
        Ok(Self {
            location: Location {
                source: r.get(0)?,
                source_hash: r.get(1)?,
                zone: r.get(2)?,
                entry: r.get::<_, Option<EvidenceId>>(3)?,
                start: super::graph::offset(r, 4)?,
                end: super::graph::offset(r, 5)?,
                page_unverified: !r.get::<_, bool>(6)?,
            },
            predicate: r.get(7)?,
            target: r.get(8)?,
            verified: r.get(9)?,
            references: serde_json::from_str(&r.get::<_, String>(10)?)
                .map_err(|cause| Error::graph_index_invalid().caused_by(cause))?,
            references_complete: r.get(11)?,
            missing: r.get(12)?,
        })
    }
    fn problem(&self, kind: ProblemKind, reference: Option<String>) -> Problem {
        Problem {
            location: self.location.clone(),
            kind,
            extraction_issue: None,
            reference,
        }
    }
}
