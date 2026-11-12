//! Existing bounded link traversal queries.
use super::{
    Index,
    limits::{GRAPH_DEPTH_MAX, GRAPH_PAGE_LIMIT, LINK_LIMIT_MAX, LINK_LIMIT_MIN},
};
use crate::convert::usize_u32_capped;
use crate::index::queries::links::{BROKEN_LINKS, DEGREE, INCOMING_LINKS, OUTGOING_LINKS};
use crate::{Error, Result, values::Slug};
use rusqlite::params;
use std::collections::{BTreeSet, VecDeque};

impl Index {
    pub(crate) fn visit_broken_links(
        &self,
        slug: &Slug,
        mut visit: impl FnMut(String),
    ) -> Result<()> {
        let mut statement = self.connection.prepare(BROKEN_LINKS)?;
        for target in statement.query_map([slug], |row| row.get::<_, String>(0))? {
            visit(target?);
        }
        Ok(())
    }

    pub fn links(&self, slug: &Slug, incoming: bool) -> Result<Vec<Slug>> {
        self.links_page(slug, incoming, None, LINK_LIMIT_MAX)
    }
    pub fn links_page(
        &self,
        slug: &Slug,
        incoming: bool,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Slug>> {
        if !(LINK_LIMIT_MIN..=LINK_LIMIT_MAX).contains(&limit) {
            return Err(Error::invalid_link_limit());
        }
        if let Some(after) = after {
            after.parse::<Slug>()?;
        }
        let query = if incoming {
            INCOMING_LINKS
        } else {
            OUTGOING_LINKS
        };
        // The validated link bound fits u32; conversion stays after SQL preparation.
        Ok(self
            .connection
            .prepare(query)?
            .query_map(params![slug, after, usize_u32_capped(limit)], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn graph(&self, slug: &Slug, depth: usize) -> Result<Vec<Slug>> {
        if depth > GRAPH_DEPTH_MAX {
            return Err(Error::graph_depth_limit());
        }
        self.document(slug)?;
        let mut visited = BTreeSet::from([slug.to_owned()]);
        let mut queue = VecDeque::from([(slug.to_owned(), 0)]);
        while let Some((current, level)) = queue.pop_front() {
            if level == depth {
                continue;
            }
            let degree: i64 = self
                .connection
                .query_row(DEGREE, [&current], |r| r.get(0))?;
            if degree > i64::from(GRAPH_PAGE_LIMIT) {
                return Err(Error::graph_page_limit());
            }
            for neighbor in self
                .links(&current, false)?
                .into_iter()
                .chain(self.links(&current, true)?)
            {
                if !visited.contains(&neighbor) && visited.len() >= usize::from(GRAPH_PAGE_LIMIT) {
                    return Err(Error::graph_page_limit());
                }
                if visited.insert(neighbor.clone()) {
                    queue.push_back((neighbor, level + 1));
                }
            }
        }
        Ok(visited.into_iter().collect())
    }
}
