pub(in crate::index) const GENERATION: &str =
    "SELECT identity,revision FROM index_generation WHERE singleton=1";
pub(in crate::index) const INCOMPLETE: &str =
    "SELECT count(*) FROM relation_coverage WHERE current_incomplete=1 AND (?2 OR source=?1)";
pub(in crate::index) const INCOMING: &str = "SELECT source FROM links WHERE target=?1 AND (?2 IS NULL OR source>?2)
    UNION SELECT source FROM relation_occurrences WHERE target=?1 AND zone='compiled' AND (?2 IS NULL OR source>?2)
    ORDER BY source LIMIT ?3";
pub(in crate::index) const OUTGOING: &str = "SELECT target FROM links WHERE source=?1 AND (?2 IS NULL OR target>?2)
    UNION SELECT target FROM relation_occurrences WHERE source=?1 AND zone='compiled' AND (?2 IS NULL OR target>?2)
    ORDER BY target LIMIT ?3";
pub(in crate::index) const ORDINARY: &str =
    "SELECT kind FROM links WHERE source=?1 AND target=?2 ORDER BY kind";
pub(in crate::index) const TYPED: &str = "SELECT DISTINCT predicate,verified FROM relation_occurrences WHERE source=?1 AND target=?2 AND zone='compiled' ORDER BY predicate,verified";
pub(in crate::index) const CONNECTED: &str = "SELECT EXISTS(SELECT 1 FROM links WHERE source=?1) OR EXISTS(SELECT 1 FROM links WHERE target=?1)
    OR EXISTS(SELECT 1 FROM relation_occurrences WHERE source=?1 AND zone='compiled')
    OR EXISTS(SELECT 1 FROM relation_occurrences WHERE target=?1 AND zone='compiled')";

pub(in crate::index) const PAGE_UNVERIFIED: &str = "SELECT NOT verified FROM pages WHERE slug=?1";
