//! Derived embedding writes, entered only through Comb's mutation checks.
use super::Index;
use crate::index::queries::embedding_write::{
    ACTIVATE_SPACE, CHUNK_EXISTS, CLEAR_CHUNK_FAILURE, CLEAR_FAILURES, DEACTIVATE_SPACE,
    EMBEDDING_FINDINGS, INSERT_VECTOR, REJECT_CHUNK,
};
use crate::{
    Result,
    embedding::{Applied, Batch, Lease, Space},
    search::pages::{FINDING_LIMIT, Finding},
};
use rusqlite::params;

impl Index {
    pub(crate) fn restore_space(&mut self, space: &Space) -> Result<()> {
        space.validate()?;
        if let Some((_, existing)) = self.active_space()? {
            if &existing != space {
                return Err(crate::Error::invalid_active_space());
            }
            return Ok(());
        }
        let lease = Lease {
            fingerprint: space.fingerprint()?,
            activation: crate::values::ActivationId::generate(),
            dimensions: space.dimensions(),
        };
        self.activate_embeddings(&lease, space)
    }
    pub(crate) fn deactivate_embeddings(&mut self) -> Result<()> {
        let tx = self.connection.transaction()?;
        tx.execute(DEACTIVATE_SPACE, [])?;
        tx.execute(CLEAR_FAILURES, [])?;
        tx.commit()?;
        // The content-addressed vector cache remains reusable after reactivation.
        Ok(())
    }
    pub(crate) fn activate_embeddings(&mut self, lease: &Lease, space: &Space) -> Result<()> {
        let tx = self.connection.transaction()?;
        activate(&tx, lease, space)?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn apply_embeddings(
        &mut self,
        batch: Batch,
        normalized: Vec<Result<Vec<f32>>>,
    ) -> Result<Applied> {
        let tx = self.connection.transaction()?;
        let mut applied = Applied::default();
        let mut published_hashes = Vec::new();
        for (job, vector) in batch.jobs.into_iter().zip(normalized) {
            let exists: bool = tx.query_row(
                CHUNK_EXISTS,
                params![job.id, job.instance, job.input_hash],
                |r| r.get(0),
            )?;
            if !exists {
                applied.stale += 1;
                continue;
            }
            match vector {
                Ok(vector) => {
                    let bytes: Vec<_> = vector.into_iter().flat_map(f32::to_le_bytes).collect();
                    tx.execute(
                        INSERT_VECTOR,
                        params![batch.lease.fingerprint, job.input_hash, bytes],
                    )?;
                    tx.execute(CLEAR_CHUNK_FAILURE, [job.id])?;
                    published_hashes.push(job.input_hash);
                    applied.embedded += 1;
                }
                Err(error) => {
                    tx.execute(
                        REJECT_CHUNK,
                        params![
                            job.id,
                            batch.lease.fingerprint,
                            crate::error::json_text(&error)?
                        ],
                    )?;
                    applied.rejected += 1;
                }
            }
        }
        // Both databases are derived. A cross-WAL power loss may lose a cache
        // insertion or a failure marker, causing recomputation, never authority loss.
        tx.commit()?;
        // Publish only committed immutable rows. In particular, a failed cache
        // transaction cannot leave a resident value shadowing a later retry.
        if let Some(cache) = &self.vectors
            && cache.matches(&batch.lease)
        {
            for hash in published_hashes {
                self.cached_vector(cache, &hash)?;
            }
        }
        Ok(applied)
    }
    pub(crate) fn retry_embedding_failures(&mut self) -> Result<usize> {
        Ok(self.connection.execute(CLEAR_FAILURES, [])?)
    }
    pub(crate) fn embedding_findings(&self) -> Result<Vec<Finding>> {
        Ok(self
            .connection
            .prepare(EMBEDDING_FINDINGS)?
            .query_map([FINDING_LIMIT + 1], |r| {
                Ok(Finding {
                    slug: r.get(0)?,
                    kind: crate::values::FindingKind::EmbeddingRejected,
                    detail: format!("Chunk {}: {}", r.get::<_, u32>(1)?, r.get::<_, String>(2)?),
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
}

pub(super) fn activate(tx: &rusqlite::Transaction<'_>, lease: &Lease, space: &Space) -> Result<()> {
    tx.execute(CLEAR_FAILURES, [])?;
    tx.execute(
        ACTIVATE_SPACE,
        params![
            lease.fingerprint,
            lease.activation,
            crate::error::json_text(space)?
        ],
    )?;
    Ok(())
}
