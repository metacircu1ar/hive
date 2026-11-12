//! Search selection and graph selection intersect; neither can widen the other.
use crate::{graph::Query, search::Filter};
use rusqlite::{Statement, ToSql};

pub(super) fn bind(
    statement: &mut Statement<'_>,
    request: &Query,
    search: Filter<'_>,
    field: &str,
    value: &dyn ToSql,
) -> rusqlite::Result<()> {
    crate::index::search::bind(
        statement,
        request.filter(),
        &[
            (":search_kind", &search.kind),
            (":search_tag", &search.tag),
            (":search_prefix", &search.prefix),
            (":search_after", &search.after),
            (":search_before", &search.before),
            (field, value),
        ],
    )
}

pub(super) fn unfiltered(filter: Filter<'_>) -> bool {
    filter.kind.is_none()
        && filter.tag.is_none()
        && filter.prefix.is_none()
        && filter.after.is_none()
        && filter.before.is_none()
}
