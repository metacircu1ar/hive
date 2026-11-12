//! Exact SQL for snapshot operations.
pub(in crate::index) const BEGIN: &str = "BEGIN";

pub(in crate::index) const PIN: &str = "SELECT (SELECT count(*) FROM embedding_space),(SELECT \
            count(*) FROM \
    vector_cache.sqlite_schema)";

pub(in crate::index) const ROLLBACK: &str = "ROLLBACK";
