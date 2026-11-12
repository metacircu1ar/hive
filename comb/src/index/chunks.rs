//! Chunk and full-text rows inserted within page replacement transactions.
use super::Index;
use crate::index::queries::chunks::{INSERT_CHUNK, INSERT_CHUNK_FTS};
use crate::values::{ContentHash, Slug, Zone};
use crate::{Error, Page, Result, chunk};
use rusqlite::{Connection, params};

impl Index {
    pub(super) fn insert_chunks(db: &Connection, slug: &Slug, page: &Page) -> Result<()> {
        let meta = &page.metadata;
        let mut chunks = chunk::chunks(page);
        if chunks.is_empty() {
            chunks.push(chunk::Chunk {
                zone: Zone::Compiled,
                heading: String::new(),
                text: meta.authored.title.clone(),
                verified: page.verified(),
            });
        }
        for (ordinal, c) in chunks.iter().enumerate() {
            let ordinal =
                i64::try_from(ordinal).map_err(|cause| Error::chunk_limit().caused_by(cause))?;
            let input = format!("{}\n{}\n{}", meta.authored.title, c.heading, c.text);
            db.prepare_cached(INSERT_CHUNK)?.execute(params![
                slug,
                ordinal,
                c.zone,
                c.heading,
                c.text,
                c.verified,
                ContentHash::digest(input),
                uuid::Uuid::new_v4().to_string()
            ])?;
            db.prepare_cached(INSERT_CHUNK_FTS)?.execute(params![
                db.last_insert_rowid(),
                meta.authored.title,
                c.heading,
                c.text,
                slug
            ])?;
        }
        Ok(())
    }
}
