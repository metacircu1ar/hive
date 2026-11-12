//! Graph reads stay on the caller's pinned index snapshot.
macro_rules! filter {
    ($prefix:literal) => {
        concat!(
            "(:",
            $prefix,
            "kind IS NULL OR p.kind=:",
            $prefix,
            "kind) AND ",
            "(:",
            $prefix,
            "tag IS NULL OR EXISTS(SELECT 1 FROM tags WHERE slug=p.slug AND tag=:",
            $prefix,
            "tag)) AND ",
            "(:",
            $prefix,
            "prefix IS NULL OR substr(p.slug,1,length(:",
            $prefix,
            "prefix))=:",
            $prefix,
            "prefix) AND ",
            "(:",
            $prefix,
            "after IS NULL OR julianday(p.updated_at)>=julianday(:",
            $prefix,
            "after)) AND ",
            "(:",
            $prefix,
            "before IS NULL OR julianday(p.updated_at)<=julianday(:",
            $prefix,
            "before))"
        )
    };
}
pub(in crate::index) const GENERATION: &str =
    "SELECT identity,revision FROM index_generation WHERE singleton=1";
pub(in crate::index) const NODE: &str = concat!(
    "SELECT p.slug,p.content_hash,p.verified FROM pages p WHERE p.slug=:slug AND ",
    filter!(""),
    " AND ",
    filter!("search_")
);
// Count-only SQLite operations use compact indexes, not page JSON payloads.
pub(in crate::index) const PROJECTION_COMPLETE: &str =
    "SELECT (SELECT count(*) FROM pages)=(SELECT count(*) FROM relation_coverage)";
// Equal counts plus membership prove a complete projection. Both sides have
// compact slug indexes; never read page JSON to validate this invariant.
pub(in crate::index) const PROJECTION_MEMBERSHIP: &str = "SELECT NOT EXISTS(SELECT 1 FROM pages p \
    WHERE NOT EXISTS(SELECT 1 \
     FROM relation_coverage c WHERE c.source=p.slug))";
pub(in crate::index) const INCOMPLETE: &str = concat!(
    "SELECT count(*) FROM relation_coverage c INDEXED BY relation_coverage_current \
     WHERE c.current_incomplete=1 AND EXISTS(SELECT 1 FROM pages p WHERE \
     p.slug=c.source AND (:reviewed=0 OR p.verified=1) AND ",
    filter!(""),
    " AND ",
    filter!("search_"),
    ")"
);
pub(in crate::index) const ALL_TOTALS: &str = "SELECT count(*),coalesce(sum(current_count),0),\
    coalesce(sum(historical_count),0),\
     coalesce(sum(historical_incomplete),0) FROM relation_coverage INDEXED BY \
    relation_coverage_totals";
pub(in crate::index) const FILTERED_TOTALS: &str = concat!(
    "SELECT count(*),coalesce(sum(c.current_count),0),coalesce(sum(c.historical_count),0),\
     coalesce(sum(c.historical_incomplete),0) \
     FROM pages p JOIN relation_coverage c ON c.source=p.slug WHERE \
     (:reviewed=0 OR p.verified=1) AND ",
    filter!(""),
    " AND ",
    filter!("search_")
);
pub(in crate::index) const ADJACENCY: &str = "SELECT source,ordinal,content_hash,start,end,\
    predicate,target,verified \
     FROM relation_occurrences WHERE zone='compiled' AND \
     predicate IN (?2,?3,?4,?5) AND (?6=0 OR verified=1) AND \
     ((source=?1 AND (?7!='incoming' OR predicate='related_to')) OR \
      (target=?1 AND (?7!='outgoing' OR predicate='related_to'))) \
     ORDER BY CASE WHEN source=?1 THEN target ELSE source END,predicate,source,start,ordinal \
     LIMIT ?8";
pub(in crate::index) const PAGE: &str = "SELECT content_hash,page_json FROM pages WHERE slug=?1";
pub(in crate::index) const REFERENCES: &str = "SELECT evidence_refs,references_complete FROM \
    relation_occurrences WHERE source=?1 AND \
    ordinal=?2";
pub(in crate::index) const EXISTS: &str = "SELECT EXISTS(SELECT 1 FROM pages WHERE slug=?1)";
