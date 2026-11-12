//! Frozen pre-S7 query expressions; compare actual Rust-assembled bytes.
pub(super) mod pages {
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

    pub(in crate::index) const LIST: &str = "SELECT slug,title,kind,content_hash,verified FROM pages p WHERE (?1 IS \
    NULL OR kind=?1) AND (?2 IS NULL OR EXISTS(SELECT 1 FROM tags t WHERE \
    t.slug=p.slug AND t.tag=?2)) AND (?3 IS NULL OR \
    substr(slug,1,length(?3))=?3) AND (?4=0 OR verified=0) AND (?5 IS NULL OR \
    slug>?5) ORDER BY slug LIMIT ?6";

    pub(in crate::index) const STATS: &str = "SELECT (SELECT count(*) FROM pages),(SELECT count(*) FROM chunks),(SELECT \
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
}
pub(super) mod chunks {
    pub(in crate::index) const INSERT_CHUNK: &str = "INSERT INTO \
    chunks(slug,ordinal,zone,heading,text,verified,input_hash,instance) \
    VALUES(?1,?2,?3,?4,?5,?6,?7,?8)";

    pub(in crate::index) const INSERT_CHUNK_FTS: &str =
        "INSERT INTO chunk_fts(rowid,title,heading,text,slug) VALUES(?1,?2,?3,?4,?5)";
}
pub(super) mod links {
    pub(in crate::index) const BROKEN_LINKS: &str = "SELECT DISTINCT target FROM links WHERE source=?1 AND NOT \
    EXISTS(SELECT 1 FROM pages WHERE slug=target) ORDER BY target";

    pub(in crate::index) const INCOMING_LINKS: &str = "SELECT DISTINCT source FROM links WHERE target=?1 AND (?2 IS \
    NULL OR source>?2) ORDER BY source LIMIT ?3";

    pub(in crate::index) const OUTGOING_LINKS: &str = "SELECT DISTINCT target FROM links WHERE source=?1 AND (?2 IS \
    NULL OR target>?2) ORDER BY target LIMIT ?3";

    pub(in crate::index) const DEGREE: &str = "SELECT count(*) FROM (SELECT target AS node FROM links WHERE source=?1 \
    UNION SELECT source FROM links WHERE target=?1 LIMIT 1001)";
}
pub(super) mod embedding {
    pub(in crate::index) const JOBS: &str = "SELECT c.id,c.instance,c.input_hash,\
p.title||char(10)||c.heading||char(10)||c.text,c.slug,c.ordinal FROM chunks c JOIN pages p ON \
p.slug=c.slug\n             WHERE c.id>?3 AND NOT EXISTS(SELECT 1 FROM vector_cache.vectors v \
WHERE v.fingerprint=?1 AND v.input_hash=c.input_hash)\n             AND NOT EXISTS(SELECT 1 \
FROM embedding_failures f WHERE f.chunk_id=c.id AND f.fingerprint=?1)\n             ORDER BY \
c.id LIMIT ?2";

    pub(in crate::index) const ACTIVE_SPACE: &str =
        "SELECT fingerprint,activation,specification FROM embedding_space WHERE singleton=1";
}
pub(super) mod embedding_write {
    pub(in crate::index) const CLEAR_FAILURES: &str = "DELETE FROM embedding_failures";

    pub(in crate::index) const ACTIVATE_SPACE: &str = "INSERT INTO embedding_space VALUES(1,?1,?2,?3) ON \
CONFLICT(singleton) DO UPDATE SET fingerprint=excluded.fingerprint,\
activation=excluded.activation,specification=excluded.specification";

    pub(in crate::index) const CHUNK_EXISTS: &str =
        "SELECT EXISTS(SELECT 1 FROM chunks WHERE id=?1 AND instance=?2 AND input_hash=?3)";

    pub(in crate::index) const INSERT_VECTOR: &str = "INSERT INTO vector_cache.vectors VALUES(?1,?2,?3) ON \
    CONFLICT(fingerprint,input_hash) DO NOTHING";

    pub(in crate::index) const CLEAR_CHUNK_FAILURE: &str =
        "DELETE FROM embedding_failures WHERE chunk_id=?1";

    pub(in crate::index) const REJECT_CHUNK: &str = "INSERT INTO embedding_failures VALUES(?1,?2,?3) ON \
    CONFLICT(chunk_id) DO UPDATE SET fingerprint=excluded.fingerprint,error=excluded.error";

    pub(in crate::index) const EMBEDDING_FINDINGS: &str = "SELECT c.slug,c.ordinal,f.error FROM embedding_failures f \
    JOIN chunks c ON c.id=f.chunk_id JOIN embedding_space s ON s.fingerprint=f.fingerprint \
    ORDER BY c.slug,c.ordinal LIMIT ?1";
    pub(in crate::index) const DEACTIVATE_SPACE: &str = "DELETE FROM embedding_space";
}
pub(super) mod cache {
    pub(in crate::index) const WARM_VECTORS: &str = "SELECT v.input_hash,v.vector FROM (SELECT DISTINCT input_hash FROM \
    chunks) c JOIN vector_cache.vectors v ON v.input_hash=c.input_hash WHERE v.fingerprint=?1 \
    LIMIT ?2";

    pub(in crate::index) const CACHED_VECTOR: &str =
        "SELECT vector FROM vector_cache.vectors WHERE fingerprint=?1 AND input_hash=?2";
}
pub(super) mod snapshot {
    pub(in crate::index) const BEGIN: &str = "BEGIN";

    pub(in crate::index) const PIN: &str = "SELECT (SELECT count(*) FROM embedding_space),(SELECT count(*) FROM \
    vector_cache.sqlite_schema)";

    pub(in crate::index) const ROLLBACK: &str = "ROLLBACK";
}
pub(super) mod schema {
    pub(in crate::index) const ATTACH_VECTORS: &str = "ATTACH DATABASE ?1 AS vector_cache";

    pub(in crate::index) const QUERY_ONLY: &str = "PRAGMA query_only=ON";

    pub(in crate::index) const WRITER_PRAGMAS: &str =
        "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;";

    pub(in crate::index) const SCHEMA_OBJECTS: &str = "SELECT type,name,coalesce(sql,'') FROM sqlite_schema WHERE name \
    NOT LIKE 'sqlite_%' ORDER BY type,name";
    pub(in crate::index) const QUICK_CHECK: &str = "PRAGMA quick_check";
    pub(in crate::index) const EMBEDDING_SPACE_DEFINITION: &str =
        "SELECT sql FROM sqlite_schema WHERE type='table' AND name='embedding_space'";
    pub(in crate::index) const BEGIN_IMMEDIATE: &str = "BEGIN IMMEDIATE";
    pub(in crate::index) const COMMIT: &str = "COMMIT";
    pub(in crate::index) const ROLLBACK: &str = "ROLLBACK";
}
pub(super) mod search {
    //! Compile-time query assembly keeps every retrieval route distinct.
    macro_rules! filter {
        () => {
            "(:kind IS NULL OR p.kind=:kind) AND (:tag IS NULL OR EXISTS(SELECT 1 FROM \
            tags WHERE slug=p.slug AND tag=:tag)) AND (:prefix IS NULL OR \
            substr(p.slug,1,length(:prefix))=:prefix) AND (:after IS NULL OR \
            julianday(p.updated_at)>=julianday(:after)) AND (:before IS NULL OR \
            julianday(p.updated_at)<=julianday(:before))"
        };
    }
    macro_rules! columns {
        () => {
            "c.id,p.slug,c.zone='compiled',c.verified,instr(p.title_key,:lower)>0,\
        unixepoch(p.updated_at),(p.title_key=:lower OR p.slug=:lower)"
        };
    }

    pub(in crate::index) const FTS: &str = concat!(
        "SELECT ",
        columns!(),
        " FROM chunk_fts JOIN chunks c ON c.id=chunk_fts.rowid JOIN pages p ON \
        p.slug=c.slug WHERE chunk_fts MATCH :query AND ",
        filter!(),
        " ORDER BY bm25(chunk_fts),p.slug,c.ordinal"
    );
    pub(in crate::index) const IDENTITY_SHORT: &str = concat!(
        "SELECT ",
        columns!(),
        " FROM pages p JOIN chunks c ON c.slug=p.slug AND c.ordinal=0 WHERE \
        (instr(lower(p.slug),:query)>0 OR instr(p.title_key,:query)>0 OR \
        EXISTS(SELECT 1 FROM aliases WHERE slug=p.slug AND instr(alias,:query)>0)) \
        AND ",
        filter!(),
        " ORDER BY p.slug"
    );
    pub(in crate::index) const IDENTITY_LONG: &str = concat!(
        "SELECT ",
        columns!(),
        " FROM pages p JOIN chunks c ON c.slug=p.slug AND c.ordinal=0 WHERE p.slug \
        IN (SELECT slug FROM identity_fts WHERE identity_fts MATCH :query) AND ",
        filter!(),
        " ORDER BY p.slug"
    );
    pub(in crate::index) const EXACT: &str = concat!(
        "SELECT ",
        columns!(),
        " FROM pages p JOIN chunks c ON c.slug=p.slug AND c.ordinal=0 WHERE \
        (p.slug=:query OR p.title_key=:query OR EXISTS(SELECT 1 FROM aliases WHERE \
        slug=p.slug AND alias=:query)) AND ",
        filter!()
    );
    pub(in crate::index) const VECTOR_HASH: &str = concat!(
        "SELECT ",
        columns!(),
        ",v.input_hash FROM chunks c JOIN pages p ON p.slug=c.slug JOIN \
        vector_cache.vectors v ON v.input_hash=c.input_hash AND \
        v.fingerprint=:fingerprint WHERE ",
        filter!(),
        " ORDER BY c.slug,c.ordinal"
    );
    pub(in crate::index) const VECTOR_BLOB: &str = concat!(
        "SELECT ",
        columns!(),
        ",v.vector FROM chunks c JOIN pages p ON p.slug=c.slug JOIN \
        vector_cache.vectors v ON v.input_hash=c.input_hash AND \
        v.fingerprint=:fingerprint WHERE ",
        filter!(),
        " ORDER BY c.slug,c.ordinal"
    );
    pub(in crate::index) const COVERAGE: &str = concat!(
        "SELECT count(*),coalesce(sum(v.input_hash IS NOT NULL),0),coalesce(sum(v.input_hash IS \
        NULL AND f.chunk_id IS NOT NULL),0)\n            FROM chunks c JOIN pages p ON \
        p.slug=c.slug\n            LEFT JOIN vector_cache.vectors v ON \
        v.input_hash=c.input_hash AND v.fingerprint=:fingerprint\n            LEFT JOIN \
        embedding_failures f ON f.chunk_id=c.id AND f.fingerprint=:fingerprint WHERE ",
        filter!()
    );
    pub(in crate::index) const HYDRATE: &str = "SELECT \
    p.title,p.kind,c.text,c.heading,c.zone,c.verified,p.verified FROM chunks c JOIN pages p \
    ON p.slug=c.slug WHERE c.id=?1";
}

pub(super) mod graph;

pub(super) mod link_report;

pub(super) mod graph_diagnostics;
