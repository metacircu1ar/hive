//! Bounded BFS over current assertions, with a separate provenance hydration step.
use super::Index;
use super::pages::count;
use crate::{
    Error, Result,
    graph::{self, Cancellation, Coverage, Generation, Incomplete, Node, Query, Report, Work},
    search::Filter,
    values::{ContentHash, RelationPredicate, Slug},
};
use std::{collections::BTreeMap, rc::Rc};

mod budget;
mod coverage;
mod evidence;
mod scope;
use super::queries::graph as sql;
#[cfg(test)]
mod tests;
mod walk;
use budget::Stop;

#[derive(Clone)]
struct Row {
    source: Slug,
    ordinal: u32,
    hash: ContentHash,
    start: usize,
    end: usize,
    predicate: RelationPredicate,
    target: Slug,
    verified: bool,
}
impl Row {
    fn decode(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            source: r.get(0)?,
            ordinal: r.get(1)?,
            hash: r.get(2)?,
            start: offset(r, 3)?,
            end: offset(r, 4)?,
            predicate: r.get(5)?,
            target: r.get(6)?,
            verified: r.get(7)?,
        })
    }
    fn neighbor(&self, current: &Slug) -> &Slug {
        if &self.source == current {
            &self.target
        } else {
            &self.source
        }
    }
    fn key(&self) -> (Slug, RelationPredicate, Slug) {
        (self.source.clone(), self.predicate, self.target.clone())
    }
}
pub(super) fn offset(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<usize> {
    usize::try_from(row.get::<_, i64>(column)?).map_err(|cause| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Integer,
            Box::new(cause),
        )
    })
}

impl Index {
    // Callers must own a Snapshot, so coverage, topology and citations share a generation.
    pub(crate) fn graph_query(
        &self,
        request: &Query,
        cancellation: Cancellation,
    ) -> Result<Report> {
        self.graph_query_in_scope(request, cancellation, None)
    }
    pub(crate) fn graph_query_in_scope(
        &self,
        request: &Query,
        cancellation: Cancellation,
        search_filter: Option<Filter<'_>>,
    ) -> Result<Report> {
        request.validate()?;
        let scope = search_filter.unwrap_or_default();
        scope.validate()?;
        let generation = self.connection.query_row(sql::GENERATION, [], |r| {
            Ok(Generation {
                identity: r.get(0)?,
                revision: count(r, 1)?,
            })
        })?;
        let mut report = Report {
            request: request.clone(),
            search_filter: search_filter.map(Into::into),
            generation,
            nodes: Vec::new(),
            relations: Vec::new(),
            connection: None,
            coverage: Coverage::default(),
            work: Work::default(),
            complete: true,
            incomplete_reasons: Vec::new(),
        };
        let stop = Stop::new(request.deadline_ms, cancellation);
        let handler = stop.clone();
        self.connection
            .progress_handler(1000, Some(move || handler.reason().is_some()))?;
        let result = self.run_graph_query(request, scope, &stop, &mut report);
        let cleanup = self.connection.progress_handler(0, None::<fn() -> bool>);
        if let Err(error) = cleanup {
            if let Err(primary) = result {
                tracing::error!(name: "comb.graph_progress_release_failed",
                    error = &error as &(dyn std::error::Error + 'static),
                    decision = "discard_reader");
                return Err(primary);
            }
            return Err(error.into());
        }
        if let Err(error) = result {
            if let Some(reason) = stop.interruption(&error) {
                incomplete(&mut report, reason);
            } else {
                return Err(error);
            }
        }
        if stop.reason() == Some(Incomplete::Cancelled) {
            incomplete(&mut report, Incomplete::Cancelled);
        }
        if let Some(path) = &mut report.connection {
            path.shortest = report.complete;
        }
        if request.strict && !report.complete {
            return Err(Error::graph_incomplete());
        }
        Ok(report)
    }

    fn run_graph_query(
        &self,
        request: &Query,
        scope: Filter<'_>,
        stop: &Stop,
        report: &mut Report,
    ) -> Result<()> {
        walk::run(self, request, scope, stop, report)?;
        if stopped(report, stop) {
            return Ok(());
        }
        self.graph_validate_membership()?;
        if stopped(report, stop) {
            return Ok(());
        }
        // Completeness is settled before optional accounting spends the rest
        // of the deadline. Cancellation remains observable in either phase.
        report.coverage.totals = self.graph_optional_totals(request, scope, stop)?;
        Ok(())
    }

    fn graph_exists(&self, slug: &Slug) -> Result<bool> {
        Ok(self
            .connection
            .query_row(sql::EXISTS, [slug], |r| r.get(0))?)
    }
    fn graph_node(&self, request: &Query, scope: Filter<'_>, slug: &Slug) -> Result<Option<Node>> {
        let mut statement = self.connection.prepare_cached(sql::NODE)?;
        scope::bind(&mut statement, request, scope, ":slug", slug)?;
        Ok(statement
            .raw_query()
            .mapped(|r| {
                Ok(Node {
                    slug: r.get(0)?,
                    content_hash: r.get(1)?,
                    page_unverified: !r.get::<_, bool>(2)?,
                })
            })
            .next()
            .transpose()?)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Path reconstruction borrows the target, predecessor map, query, stop and report"
    )]
    fn connection_path(
        &self,
        target: &Slug,
        predecessors: &BTreeMap<Rc<Slug>, (Rc<Slug>, Row)>,
        request: &Query,
        stop: &Stop,
        report: &mut Report,
    ) -> Result<()> {
        let mut current = target;
        let mut path = Vec::new();
        while current != &request.anchor {
            let (parent, row) = predecessors
                .get(current)
                .ok_or_else(Error::graph_index_invalid)?;
            path.push((current.clone(), row));
            current = parent.as_ref();
        }
        let mut nodes = vec![request.anchor.clone()];
        let mut edges = Vec::new();
        for (node, row) in path.into_iter().rev() {
            if stopped(report, stop) {
                return Ok(());
            }
            let Some(edge) = self.graph_evidence(row, request, report)? else {
                return Ok(());
            };
            nodes.push(node);
            edges.push(edge);
        }
        report.connection = Some(graph::Path {
            nodes,
            unverified: edges.iter().any(|e| e.declaration.unverified),
            edges,
            shortest: report.complete,
        });
        Ok(())
    }
}
fn incomplete(report: &mut Report, reason: Incomplete) {
    report.complete = false;
    if !report.incomplete_reasons.contains(&reason) {
        report.incomplete_reasons.push(reason);
    }
}
fn stopped(report: &mut Report, stop: &Stop) -> bool {
    if let Some(reason) = stop.reason() {
        incomplete(report, reason);
        true
    } else {
        false
    }
}
