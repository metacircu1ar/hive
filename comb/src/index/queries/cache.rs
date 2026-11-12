//! Exact SQL for cache operations.
pub(in crate::index) const WARM_VECTORS: &str = "SELECT v.input_hash,v.vector FROM (SELECT \
            DISTINCT input_hash FROM \
    chunks) c JOIN vector_cache.vectors v ON v.input_hash=c.input_hash WHERE \
            v.fingerprint=?1 \
    LIMIT ?2";

pub(in crate::index) const CACHED_VECTOR: &str =
    "SELECT vector FROM vector_cache.vectors WHERE fingerprint=?1 AND input_hash=?2";
