pub(in crate::index) const GENERATION: &str =
    "SELECT identity,revision FROM index_generation WHERE singleton=1";
pub(in crate::index) const COVERAGE: &str =
    "SELECT c.source,p.content_hash,p.verified,c.current_incomplete,c.historical_incomplete,\
    c.diagnostics,c.diagnostics_omitted
    FROM relation_coverage c JOIN pages p ON p.slug=c.source ORDER BY c.source";
pub(in crate::index) const OCCURRENCES: &str =
    "SELECT r.source,r.content_hash,r.zone,r.entry,r.start,r.end,p.verified,r.predicate,\
    r.target,r.verified,r.evidence_refs,r.references_complete,
    NOT EXISTS(SELECT 1 FROM pages WHERE slug=r.target)
    FROM relation_occurrences r JOIN pages p ON p.slug=r.source ORDER BY r.source,r.ordinal";
