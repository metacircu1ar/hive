//! Union pagination does not change classic graph adjacency or degree checks.
use super::queries::link_report as sql;
use super::{Index, pages::count};
use crate::{
    Error, Result,
    graph::Generation,
    links::{Reference, Report, Typed},
    values::Slug,
};
use rusqlite::{OptionalExtension, params};

impl Index {
    pub fn link_report(
        &self,
        slug: &Slug,
        incoming: bool,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Report> {
        if !(super::limits::LINK_LIMIT_MIN..=super::limits::LINK_LIMIT_MAX).contains(&limit) {
            return Err(Error::invalid_link_limit());
        }
        if let Some(after) = after {
            after.parse::<Slug>()?;
        }
        self.graph_coverage_integrity()?;
        let incomplete =
            self.connection
                .query_row(sql::INCOMPLETE, params![slug, incoming], |r| count(r, 0))?;
        let generation = self.connection.query_row(sql::GENERATION, [], |r| {
            Ok(Generation {
                identity: r.get(0)?,
                revision: count(r, 1)?,
            })
        })?;
        let query = if incoming {
            sql::INCOMING
        } else {
            sql::OUTGOING
        };
        let mut neighbors = self
            .connection
            .prepare_cached(query)?
            .query_map(
                params![slug, after, crate::convert::usize_u32_capped(limit + 1)],
                |r| r.get::<_, Slug>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let more = neighbors.len() > limit;
        neighbors.truncate(limit);
        let next_after = more.then(|| neighbors.last().cloned()).flatten();
        let references = neighbors
            .iter()
            .map(|neighbor| self.link_reference(slug, neighbor, incoming))
            .collect::<Result<_>>()?;
        Ok(Report {
            slug: slug.clone(),
            incoming,
            generation,
            references,
            next_after,
            complete: incomplete == 0,
            current_incomplete_sources: incomplete,
        })
    }
    fn link_reference(&self, slug: &Slug, neighbor: &Slug, incoming: bool) -> Result<Reference> {
        let (source, target) = if incoming {
            (neighbor, slug)
        } else {
            (slug, neighbor)
        };
        let ordinary = self
            .connection
            .prepare_cached(sql::ORDINARY)?
            .query_map(params![source, target], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let typed = self
            .connection
            .prepare_cached(sql::TYPED)?
            .query_map(params![source, target], |r| {
                Ok(Typed {
                    predicate: r.get(0)?,
                    unverified: !r.get::<_, bool>(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let ordinary_source_page_unverified = if ordinary.is_empty() {
            None
        } else {
            self.link_page_trust(source)?
        };
        Ok(Reference {
            slug: neighbor.clone(),
            ordinary,
            ordinary_source_page_unverified,
            typed,
            page_unverified: self.link_page_trust(neighbor)?,
        })
    }
    fn link_page_trust(&self, slug: &Slug) -> Result<Option<bool>> {
        Ok(self
            .connection
            .query_row(sql::PAGE_UNVERIFIED, [slug], |r| r.get(0))
            .optional()?)
    }
    pub(crate) fn has_any_reference(&self, slug: &Slug) -> Result<bool> {
        Ok(self
            .connection
            .query_row(sql::CONNECTED, [slug], |r| r.get(0))?)
    }
}
