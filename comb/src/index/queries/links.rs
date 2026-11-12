//! Exact SQL for links operations.
pub(in crate::index) const BROKEN_LINKS: &str = "SELECT DISTINCT target FROM links WHERE \
            source=?1 AND NOT \
    EXISTS(SELECT 1 FROM pages WHERE slug=target) ORDER BY target";

pub(in crate::index) const INCOMING_LINKS: &str = "SELECT DISTINCT source FROM links WHERE \
            target=?1 AND (?2 IS \
    NULL OR source>?2) ORDER BY source LIMIT ?3";

pub(in crate::index) const OUTGOING_LINKS: &str = "SELECT DISTINCT target FROM links WHERE \
            source=?1 AND (?2 IS \
    NULL OR target>?2) ORDER BY target LIMIT ?3";

pub(in crate::index) const DEGREE: &str = "SELECT count(*) FROM (SELECT target AS node FROM \
            links WHERE source=?1 \
    UNION SELECT source FROM links WHERE target=?1 LIMIT 1001)";
