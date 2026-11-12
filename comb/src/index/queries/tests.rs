use super::*;
mod original;

#[test]
fn pages_query_bytes_match_before_relocation() {
    for (actual, expected) in [
        (pages::INSERT_PAGE, original::pages::INSERT_PAGE),
        (pages::INSERT_TAG, original::pages::INSERT_TAG),
        (pages::INSERT_ALIAS, original::pages::INSERT_ALIAS),
        (pages::INSERT_LINK, original::pages::INSERT_LINK),
        (pages::INSERT_IDENTITY, original::pages::INSERT_IDENTITY),
        (pages::DELETE_CHUNK_FTS, original::pages::DELETE_CHUNK_FTS),
        (pages::DELETE_IDENTITY, original::pages::DELETE_IDENTITY),
        (pages::DELETE_PAGE, original::pages::DELETE_PAGE),
        (pages::CONTENT_HASH, original::pages::CONTENT_HASH),
        (pages::PAGE_SLUGS, original::pages::PAGE_SLUGS),
        (pages::LIST, original::pages::LIST),
        (pages::STATS, original::pages::STATS),
        (pages::DOCUMENT, original::pages::DOCUMENT),
        (
            pages::DELETE_QUARANTINED_PAGE,
            original::pages::DELETE_QUARANTINED_PAGE,
        ),
        (pages::QUARANTINE_PAGE, original::pages::QUARANTINE_PAGE),
    ] {
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
}

#[test]
fn chunks_query_bytes_match_before_relocation() {
    for (actual, expected) in [
        (chunks::INSERT_CHUNK, original::chunks::INSERT_CHUNK),
        (chunks::INSERT_CHUNK_FTS, original::chunks::INSERT_CHUNK_FTS),
    ] {
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
}

#[test]
fn links_query_bytes_match_before_relocation() {
    for (actual, expected) in [
        (links::BROKEN_LINKS, original::links::BROKEN_LINKS),
        (links::INCOMING_LINKS, original::links::INCOMING_LINKS),
        (links::OUTGOING_LINKS, original::links::OUTGOING_LINKS),
        (links::DEGREE, original::links::DEGREE),
    ] {
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
}

#[test]
fn embedding_query_bytes_match_before_relocation() {
    for (actual, expected) in [
        (embedding::JOBS, original::embedding::JOBS),
        (embedding::ACTIVE_SPACE, original::embedding::ACTIVE_SPACE),
    ] {
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
}

#[test]
fn embedding_write_query_bytes_match_before_relocation() {
    for (actual, expected) in [
        (
            embedding_write::CLEAR_FAILURES,
            original::embedding_write::CLEAR_FAILURES,
        ),
        (
            embedding_write::ACTIVATE_SPACE,
            original::embedding_write::ACTIVATE_SPACE,
        ),
        (
            embedding_write::CHUNK_EXISTS,
            original::embedding_write::CHUNK_EXISTS,
        ),
        (
            embedding_write::INSERT_VECTOR,
            original::embedding_write::INSERT_VECTOR,
        ),
        (
            embedding_write::CLEAR_CHUNK_FAILURE,
            original::embedding_write::CLEAR_CHUNK_FAILURE,
        ),
        (
            embedding_write::REJECT_CHUNK,
            original::embedding_write::REJECT_CHUNK,
        ),
        (
            embedding_write::EMBEDDING_FINDINGS,
            original::embedding_write::EMBEDDING_FINDINGS,
        ),
        (
            embedding_write::DEACTIVATE_SPACE,
            original::embedding_write::DEACTIVATE_SPACE,
        ),
    ] {
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
}

#[test]
fn cache_query_bytes_match_before_relocation() {
    for (actual, expected) in [
        (cache::WARM_VECTORS, original::cache::WARM_VECTORS),
        (cache::CACHED_VECTOR, original::cache::CACHED_VECTOR),
    ] {
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
}

#[test]
fn snapshot_query_bytes_match_before_relocation() {
    for (actual, expected) in [
        (snapshot::BEGIN, original::snapshot::BEGIN),
        (snapshot::PIN, original::snapshot::PIN),
        (snapshot::ROLLBACK, original::snapshot::ROLLBACK),
    ] {
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
}

#[test]
fn schema_query_bytes_match_before_relocation() {
    for (actual, expected) in [
        (schema::ATTACH_VECTORS, original::schema::ATTACH_VECTORS),
        (schema::QUERY_ONLY, original::schema::QUERY_ONLY),
        (schema::WRITER_PRAGMAS, original::schema::WRITER_PRAGMAS),
        (schema::SCHEMA_OBJECTS, original::schema::SCHEMA_OBJECTS),
        (schema::QUICK_CHECK, original::schema::QUICK_CHECK),
        (
            schema::EMBEDDING_SPACE_DEFINITION,
            original::schema::EMBEDDING_SPACE_DEFINITION,
        ),
        (schema::BEGIN_IMMEDIATE, original::schema::BEGIN_IMMEDIATE),
        (schema::COMMIT, original::schema::COMMIT),
        (schema::ROLLBACK, original::schema::ROLLBACK),
    ] {
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
}

#[test]
fn search_query_bytes_match_before_relocation() {
    for (actual, expected) in [
        (search::FTS, original::search::FTS),
        (search::IDENTITY_SHORT, original::search::IDENTITY_SHORT),
        (search::IDENTITY_LONG, original::search::IDENTITY_LONG),
        (search::EXACT, original::search::EXACT),
        (search::VECTOR_HASH, original::search::VECTOR_HASH),
        (search::VECTOR_BLOB, original::search::VECTOR_BLOB),
        (search::COVERAGE, original::search::COVERAGE),
        (search::HYDRATE, original::search::HYDRATE),
    ] {
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
}

#[test]
fn graph_query_bytes_match_before_relocation() {
    for (actual, expected) in [
        (graph::GENERATION, original::graph::GENERATION),
        (graph::NODE, original::graph::NODE),
        (
            graph::PROJECTION_COMPLETE,
            original::graph::PROJECTION_COMPLETE,
        ),
        (
            graph::PROJECTION_MEMBERSHIP,
            original::graph::PROJECTION_MEMBERSHIP,
        ),
        (graph::INCOMPLETE, original::graph::INCOMPLETE),
        (graph::ALL_TOTALS, original::graph::ALL_TOTALS),
        (graph::FILTERED_TOTALS, original::graph::FILTERED_TOTALS),
        (graph::ADJACENCY, original::graph::ADJACENCY),
        (graph::PAGE, original::graph::PAGE),
        (graph::REFERENCES, original::graph::REFERENCES),
        (graph::EXISTS, original::graph::EXISTS),
    ] {
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
}

#[test]
fn link_report_query_bytes_match_before_relocation() {
    for (actual, expected) in [
        (link_report::GENERATION, original::link_report::GENERATION),
        (link_report::INCOMPLETE, original::link_report::INCOMPLETE),
        (link_report::INCOMING, original::link_report::INCOMING),
        (link_report::OUTGOING, original::link_report::OUTGOING),
        (link_report::ORDINARY, original::link_report::ORDINARY),
        (link_report::TYPED, original::link_report::TYPED),
        (link_report::CONNECTED, original::link_report::CONNECTED),
        (
            link_report::PAGE_UNVERIFIED,
            original::link_report::PAGE_UNVERIFIED,
        ),
    ] {
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
}

#[test]
fn graph_diagnostics_query_bytes_match_before_relocation() {
    for (actual, expected) in [
        (
            graph_diagnostics::GENERATION,
            original::graph_diagnostics::GENERATION,
        ),
        (
            graph_diagnostics::COVERAGE,
            original::graph_diagnostics::COVERAGE,
        ),
        (
            graph_diagnostics::OCCURRENCES,
            original::graph_diagnostics::OCCURRENCES,
        ),
    ] {
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
}

mod hydrate_original;
#[test]
fn graph_hydration_query_bytes_match_before_wrapping() {
    assert_eq!(
        search::HYDRATE_GRAPH.as_bytes(),
        hydrate_original::HYDRATE_GRAPH.as_bytes()
    );
}
