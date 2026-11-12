//! Sparse completeness is required before traversal; optional totals come after it.
use super::{Index, Stop, count, scope, sql};
use crate::{
    Error, Result,
    graph::{CoverageTotals, Incomplete, Query},
    search::Filter,
};

impl Index {
    pub(crate) fn graph_validate_membership(&self) -> Result<()> {
        let complete: bool = self
            .connection
            .query_row(sql::PROJECTION_MEMBERSHIP, [], |r| r.get(0))?;
        if !complete {
            return Err(Error::graph_index_invalid());
        }
        Ok(())
    }

    pub(crate) fn graph_coverage_integrity(&self) -> Result<()> {
        let complete: bool = self
            .connection
            .query_row(sql::PROJECTION_COMPLETE, [], |r| r.get(0))?;
        if !complete {
            return Err(Error::graph_index_invalid());
        }
        self.graph_validate_membership()
    }

    pub(super) fn graph_incomplete_sources(
        &self,
        request: &Query,
        scope: Filter<'_>,
    ) -> Result<u64> {
        let complete: bool = self
            .connection
            .query_row(sql::PROJECTION_COMPLETE, [], |r| r.get(0))?;
        if !complete {
            return Err(Error::graph_index_invalid());
        }
        let mut statement = self.connection.prepare_cached(sql::INCOMPLETE)?;
        scope::bind(
            &mut statement,
            request,
            scope,
            ":reviewed",
            &request.reviewed_only,
        )?;
        statement
            .raw_query()
            .mapped(|r| count(r, 0))
            .next()
            .transpose()?
            .ok_or_else(Error::graph_index_invalid)
    }

    pub(super) fn graph_optional_totals(
        &self,
        request: &Query,
        scope: Filter<'_>,
        stop: &Stop,
    ) -> Result<Option<CoverageTotals>> {
        match self.graph_totals(request, scope) {
            Ok(totals) => Ok(stop.reason().is_none().then_some(totals)),
            // Accounting cannot invalidate a finished traversal. Cancellation
            // and unrelated database failures still propagate to the caller.
            Err(error) if stop.interruption(&error) == Some(Incomplete::Deadline) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn graph_totals(&self, request: &Query, scope: Filter<'_>) -> Result<CoverageTotals> {
        if !request.reviewed_only && scope::unfiltered(request.filter()) && scope::unfiltered(scope)
        {
            return Ok(self.connection.query_row(sql::ALL_TOTALS, [], totals)?);
        }
        let mut statement = self.connection.prepare_cached(sql::FILTERED_TOTALS)?;
        scope::bind(
            &mut statement,
            request,
            scope,
            ":reviewed",
            &request.reviewed_only,
        )?;
        statement
            .raw_query()
            .mapped(totals)
            .next()
            .transpose()?
            .ok_or_else(Error::graph_index_invalid)
    }
}

fn totals(row: &rusqlite::Row<'_>) -> rusqlite::Result<CoverageTotals> {
    Ok(CoverageTotals {
        pages: count(row, 0)?,
        current_occurrences: count(row, 1)?,
        historical_occurrences: count(row, 2)?,
        historical_incomplete_sources: count(row, 3)?,
    })
}
