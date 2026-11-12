use super::{Index, Row, incomplete, sql};
use crate::{
    Error, Page, Result,
    graph::{Edge, Incomplete, Query, Report, Span, Support},
    values::{ContentHash, Zone},
};
use rusqlite::params;

impl Index {
    pub(super) fn graph_evidence(
        &self,
        row: &Row,
        request: &Query,
        report: &mut Report,
    ) -> Result<Option<Edge>> {
        let (hash, json): (ContentHash, String) =
            self.connection
                .query_row(sql::PAGE, [&row.source], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let page: Page = serde_json::from_str(&json)?;
        if hash != row.hash || page.verified() != row.verified {
            return Err(Error::graph_index_invalid());
        }
        let token = page
            .compiled
            .get(row.start..row.end)
            .ok_or_else(Error::graph_index_invalid)?;
        if token != format!("[[{}::{}]]", row.predicate, row.target) {
            return Err(Error::graph_index_invalid());
        }
        if token.len() > request.max_evidence_bytes - report.work.evidence_bytes {
            incomplete(report, Incomplete::EvidenceBytes);
            return Ok(None);
        }
        report.work.evidence_bytes += token.len();
        let (refs, complete): (String, bool) =
            self.connection
                .query_row(sql::REFERENCES, params![row.source, row.ordinal], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
        let references: Vec<String> = serde_json::from_str(&refs)?;
        if references.len() > crate::relations::REFERENCE_LIMIT
            || references
                .iter()
                .any(|r| r.len() > crate::relations::REFERENCE_BYTES)
        {
            return Err(Error::graph_index_invalid());
        }
        let mut edge = Edge {
            source: row.source.clone(),
            target: row.target.clone(),
            predicate: row.predicate,
            source_hash: hash,
            page_unverified: !page.verified(),
            declaration: Span {
                zone: Zone::Compiled,
                entry: None,
                start: row.start,
                end: row.end,
                text: token.to_owned(),
                unverified: !row.verified,
            },
            support: Vec::new(),
            unresolved_references: Vec::new(),
            omitted_references: 0,
            support_complete: complete,
        };
        hydrate_support(&page, references, request, report, &mut edge);
        Ok(Some(edge))
    }
}

fn hydrate_support(
    page: &Page,
    references: Vec<String>,
    request: &Query,
    report: &mut Report,
    edge: &mut Edge,
) {
    let reviewed = page.reviewed_start();
    for reference in references {
        let Some((position, entry)) = page
            .timeline
            .iter()
            .enumerate()
            .find(|(_, e)| e.metadata.authored.id.as_str() == reference)
        else {
            edge.support_complete = false;
            if reference.len() > request.max_evidence_bytes - report.work.evidence_bytes {
                edge.omitted_references += 1;
                incomplete(report, Incomplete::EvidenceBytes);
            } else {
                report.work.evidence_bytes += reference.len();
                edge.unresolved_references.push(reference);
            }
            continue;
        };
        let bytes = reference.len() + entry.text.len();
        if bytes > request.max_evidence_bytes - report.work.evidence_bytes {
            edge.omitted_references += 1;
            edge.support_complete = false;
            incomplete(report, Incomplete::EvidenceBytes);
            continue;
        }
        report.work.evidence_bytes += bytes;
        edge.support.push(Support {
            entry: entry.metadata.authored.id.clone(),
            text: entry.text.clone(),
            unverified: !(entry.metadata.owned().actor().is_human()
                || reviewed.is_some_and(|start| position >= start)),
        });
    }
}
