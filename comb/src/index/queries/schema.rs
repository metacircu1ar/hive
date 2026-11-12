//! Exact SQL for schema operations.
pub(in crate::index) const ATTACH_VECTORS: &str = "ATTACH DATABASE ?1 AS vector_cache";

pub(in crate::index) const QUERY_ONLY: &str = "PRAGMA query_only=ON";

pub(in crate::index) const WRITER_PRAGMAS: &str =
    "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;";

pub(in crate::index) const SCHEMA_OBJECTS: &str = "SELECT type,name,coalesce(sql,'') FROM \
            sqlite_schema WHERE name \
    NOT LIKE 'sqlite_%' ORDER BY type,name";

pub(in crate::index) const QUICK_CHECK: &str = "PRAGMA quick_check";

pub(in crate::index) const EMBEDDING_SPACE_DEFINITION: &str =
    "SELECT sql FROM sqlite_schema WHERE type='table' AND name='embedding_space'";

pub(in crate::index) const BEGIN_IMMEDIATE: &str = "BEGIN IMMEDIATE";

pub(in crate::index) const COMMIT: &str = "COMMIT";

pub(in crate::index) const ROLLBACK: &str = "ROLLBACK";
