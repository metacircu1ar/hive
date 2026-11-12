//! Exact SQL for embedding write operations.
pub(in crate::index) const CLEAR_FAILURES: &str = "DELETE FROM embedding_failures";

pub(in crate::index) const ACTIVATE_SPACE: &str = "INSERT INTO embedding_space \
            VALUES(1,?1,?2,?3) ON \
CONFLICT(singleton) DO UPDATE SET fingerprint=excluded.fingerprint,\
activation=excluded.activation,specification=excluded.specification";

pub(in crate::index) const CHUNK_EXISTS: &str =
    "SELECT EXISTS(SELECT 1 FROM chunks WHERE id=?1 AND instance=?2 AND input_hash=?3)";

pub(in crate::index) const INSERT_VECTOR: &str = "INSERT INTO vector_cache.vectors \
            VALUES(?1,?2,?3) ON \
    CONFLICT(fingerprint,input_hash) DO NOTHING";

pub(in crate::index) const CLEAR_CHUNK_FAILURE: &str =
    "DELETE FROM embedding_failures WHERE chunk_id=?1";

pub(in crate::index) const REJECT_CHUNK: &str = "INSERT INTO embedding_failures \
            VALUES(?1,?2,?3) ON \
    CONFLICT(chunk_id) DO UPDATE SET fingerprint=excluded.fingerprint,error=excluded.error";

pub(in crate::index) const EMBEDDING_FINDINGS: &str = "SELECT c.slug,c.ordinal,f.error FROM \
            embedding_failures f \
    JOIN chunks c ON c.id=f.chunk_id JOIN embedding_space s ON s.fingerprint=f.fingerprint \
    ORDER BY c.slug,c.ordinal LIMIT ?1";

pub(in crate::index) const DEACTIVATE_SPACE: &str = "DELETE FROM embedding_space";
