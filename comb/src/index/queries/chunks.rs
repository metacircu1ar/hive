//! Exact SQL for chunks operations.
pub(in crate::index) const INSERT_CHUNK: &str = "INSERT INTO \
    chunks(slug,ordinal,zone,heading,text,verified,input_hash,instance) \
    VALUES(?1,?2,?3,?4,?5,?6,?7,?8)";

pub(in crate::index) const INSERT_CHUNK_FTS: &str =
    "INSERT INTO chunk_fts(rowid,title,heading,text,slug) VALUES(?1,?2,?3,?4,?5)";
