//! Exact SQL for pages operations.
pub(in crate::index) const INSERT_PAGE: &str =
    "INSERT INTO pages VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)";

pub(in crate::index) const INSERT_TAG: &str = "INSERT OR IGNORE INTO tags VALUES(?1,?2)";

pub(in crate::index) const INSERT_ALIAS: &str = "INSERT OR IGNORE INTO aliases VALUES(?1,?2)";

pub(in crate::index) const INSERT_LINK: &str = "INSERT INTO links VALUES(?1,?2,?3)";

pub(in crate::index) const INSERT_IDENTITY: &str =
    "INSERT INTO identity_fts(slug,title,aliases) VALUES(?1,?2,?3)";

pub(in crate::index) const DELETE_CHUNK_FTS: &str =
    "DELETE FROM chunk_fts WHERE rowid IN (SELECT id FROM chunks WHERE slug=?1)";

pub(in crate::index) const DELETE_IDENTITY: &str = "DELETE FROM identity_fts WHERE slug=?1";

pub(in crate::index) const DELETE_PAGE: &str = "DELETE FROM pages WHERE slug=?1";

pub(in crate::index) const CONTENT_HASH: &str = "SELECT content_hash FROM pages WHERE slug=?1";

pub(in crate::index) const PAGE_SLUGS: &str =
    "SELECT slug FROM pages UNION SELECT slug FROM quarantined_pages";

pub(in crate::index) const LIST: &str = "SELECT slug,title,kind,content_hash,verified FROM \
            pages p WHERE (?1 IS \
    NULL OR kind=?1) AND (?2 IS NULL OR EXISTS(SELECT 1 FROM tags t WHERE \
    t.slug=p.slug AND t.tag=?2)) AND (?3 IS NULL OR \
    substr(slug,1,length(?3))=?3) AND (?4=0 OR verified=0) AND (?5 IS NULL OR \
    slug>?5) ORDER BY slug LIMIT ?6";

pub(in crate::index) const STATS: &str = "SELECT (SELECT count(*) FROM pages),(SELECT count(*) \
            FROM chunks),(SELECT \
    count(*) FROM links),(SELECT count(*) FROM pages WHERE \
    inbox_pending=1),(SELECT count(*) FROM pages WHERE verified=0),(SELECT \
    count(*) FROM pages WHERE unresolved_proposal=1),(SELECT count(*) FROM \
    quarantined_pages)";

pub(in crate::index) const DOCUMENT: &str =
    "SELECT content_hash,page_json FROM pages WHERE slug=?1";

pub(in crate::index) const DELETE_QUARANTINED_PAGE: &str =
    "DELETE FROM quarantined_pages WHERE slug=?1";

pub(in crate::index) const QUARANTINE_PAGE: &str =
    "INSERT INTO quarantined_pages(slug,code) VALUES(?1,?2)";
