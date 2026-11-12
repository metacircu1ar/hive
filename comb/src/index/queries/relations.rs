//! Derived relation projection; callers own the page transaction/snapshot.
pub(in crate::index) const INSERT_OCCURRENCE: &str =
    "INSERT INTO relation_occurrences VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)";
pub(in crate::index) const INSERT_COVERAGE: &str =
    "INSERT INTO relation_coverage VALUES(?1,?2,?3,?4,?5,?6,?7,?8)";
pub(in crate::index) const ADVANCE_GENERATION: &str =
    "UPDATE index_generation SET revision=revision+1 WHERE singleton=1 RETURNING revision";
