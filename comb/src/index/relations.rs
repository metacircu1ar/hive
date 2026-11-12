//! Occurrences, extraction coverage and page contents commit together.
use super::Index;
use super::queries::relations::{INSERT_COVERAGE, INSERT_OCCURRENCE};
use crate::convert::usize_u32_capped;
use crate::{
    Page, Result, relations,
    values::{ContentHash, Slug, Zone},
};
use rusqlite::{Connection, params};

impl Index {
    pub(super) fn insert_relations(
        db: &Connection,
        slug: &Slug,
        raw: &str,
        page: &Page,
    ) -> Result<()> {
        let projection = relations::extract(page);
        let hash = ContentHash::digest(raw);
        for (ordinal, occurrence) in projection.occurrences.iter().enumerate() {
            db.prepare_cached(INSERT_OCCURRENCE)?.execute(params![
                slug,
                usize_u32_capped(ordinal),
                hash,
                occurrence.zone,
                occurrence.entry,
                usize_u32_capped(occurrence.start),
                usize_u32_capped(occurrence.end),
                occurrence.predicate,
                occurrence.target,
                occurrence.verified,
                crate::error::json_text(&occurrence.references)?,
                occurrence.references_complete,
            ])?;
        }
        let current = projection
            .occurrences
            .iter()
            .filter(|o| o.zone == Zone::Compiled)
            .count();
        db.prepare_cached(INSERT_COVERAGE)?.execute(params![
            slug,
            relations::EXTRACTION,
            usize_u32_capped(current),
            usize_u32_capped(projection.occurrences.len() - current),
            projection.current_incomplete,
            projection.historical_incomplete,
            crate::error::json_text(&projection.diagnostics)?,
            usize_u32_capped(projection.diagnostics_omitted),
        ])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
