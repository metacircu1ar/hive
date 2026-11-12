//! Exact SQL for embedding operations.
pub(in crate::index) const JOBS: &str = "SELECT c.id,c.instance,c.input_hash,\
p.title||char(10)||c.heading||char(10)||c.text,c.slug,c.ordinal FROM chunks c JOIN pages p ON \
p.slug=c.slug\n             WHERE c.id>?3 AND NOT EXISTS(SELECT 1 FROM vector_cache.vectors v \
WHERE v.fingerprint=?1 AND v.input_hash=c.input_hash)\n             AND NOT EXISTS(SELECT 1 \
FROM embedding_failures f WHERE f.chunk_id=c.id AND f.fingerprint=?1)\n             ORDER BY \
c.id LIMIT ?2";

pub(in crate::index) const ACTIVE_SPACE: &str =
    "SELECT fingerprint,activation,specification FROM embedding_space WHERE singleton=1";
