use super::*;
use crate::{
    search::{Mode, QueryVector, Search},
    values::{CreateSafety, MatchEvidence, Route},
};

fn search_request() -> Search {
    Search {
        query: "unmatched-needle".into(),
        graph: Some(request("concepts/a")),
        ..Search::default()
    }
}
#[test]
fn graph_search_finds_uncited_by_text_pages_without_embeddings_or_identity_claims() {
    let (_root, comb) = fixture();
    let req = search_request();
    let results = comb.index.search(&req).unwrap();
    assert_eq!(
        results
            .results
            .iter()
            .map(|h| h.slug.as_str())
            .collect::<Vec<_>>(),
        ["concepts/b", "concepts/c", "concepts/d"]
    );
    for hit in &results.results {
        assert_eq!(hit.matched_by, Route::Graph);
        assert_eq!(hit.evidence, MatchEvidence::Relation);
        assert_eq!(hit.create_safety, CreateSafety::Unknown);
        assert!(hit.unverified && hit.page_unverified);
    }
    let graph = results.graph.unwrap();
    assert!(graph.complete);
    assert_eq!(graph.relations.len(), 4);
    assert!(graph.search_filter.is_some());
    assert_eq!(results.mode, Mode::Text);
    assert!(results.coverage.is_none());
    let mut ordinary = req;
    ordinary.graph = None;
    assert!(comb.index.search(&ordinary).unwrap().results.is_empty());
}

#[test]
fn graph_and_search_filters_intersect_at_every_hop_and_support_distinct_tags() {
    let (_root, mut comb) = fixture();
    for (slug, tags) in [
        ("concepts/a", vec!["outer", "inner"]),
        ("concepts/b", vec!["outer"]),
        ("concepts/c", vec!["outer", "inner"]),
        ("concepts/d", vec!["inner"]),
    ] {
        let slug = slug.parse().unwrap();
        let mut doc = comb.get(&slug).unwrap();
        doc.page.metadata.authored.tags = tags.into_iter().map(str::to_owned).collect();
        comb.put_markdown(
            &slug,
            &doc.page.render().unwrap(),
            Some(&doc.hash),
            &write(&Actor::Human, &format!("tags-{slug}"), None).unwrap(),
        )
        .unwrap();
    }
    let mut req = search_request();
    req.tag = Some("outer".into());
    req.graph.as_mut().unwrap().tag = Some("inner".into());
    let result = comb.index.search(&req).unwrap();
    assert_eq!(result.results.len(), 1);
    assert_eq!(result.results[0].slug.as_str(), "concepts/c");
    let report = result.graph.unwrap();
    assert_eq!(report.relations.len(), 1);
    assert_eq!(report.search_filter.unwrap().tag.as_deref(), Some("outer"));
    assert_eq!(report.request.tag.as_deref(), Some("inner"));
    // Filters constrain both anchor and each endpoint, not just ranked output.
    req.prefix = Some("concepts/d".into());
    assert!(
        comb.index
            .search(&req)
            .unwrap()
            .graph
            .unwrap()
            .nodes
            .is_empty()
    );
    req.prefix = None;
    req.graph.as_mut().unwrap().kind = Some("incompatible".into());
    assert!(
        comb.index
            .search(&req)
            .unwrap()
            .graph
            .unwrap()
            .nodes
            .is_empty()
    );
}

#[test]
fn graph_errors_and_partial_status_survive_hybrid_text_fallback() {
    let (_root, mut comb) = fixture();
    put(
        &mut comb,
        "concepts/bad",
        "## Relations\n- [[bogus::concepts/a]]",
        false,
    );
    let mut req = search_request();
    let result = comb
        .index
        .retrieve(&req, Mode::Hybrid, QueryVector::NotRequested)
        .unwrap();
    assert!(result.fallback.is_some());
    assert_eq!(result.results.len(), 3);
    assert!(!result.graph.unwrap().complete);
    req.graph.as_mut().unwrap().strict = true;
    assert_eq!(
        comb.index
            .retrieve(&req, Mode::Hybrid, QueryVector::NotRequested)
            .err()
            .unwrap()
            .code,
        crate::ErrorCode::GraphIncomplete
    );
    req.graph.as_mut().unwrap().strict = false;
    let cancellation = Cancellation::default();
    cancellation.cancel();
    let result = comb
        .index
        .retrieve_cancellable(&req, Mode::Text, QueryVector::NotRequested, cancellation)
        .unwrap();
    assert_eq!(
        result.graph.unwrap().incomplete_reasons,
        [Incomplete::Cancelled]
    );
    req.graph.as_mut().unwrap().anchor = "concepts/absent".parse().unwrap();
    assert_eq!(
        comb.index
            .retrieve(&req, Mode::Hybrid, QueryVector::NotRequested)
            .err()
            .unwrap()
            .code,
        crate::ErrorCode::NotFound
    );
}

#[test]
fn connection_search_returns_only_cited_path_nodes_not_visited_branches() {
    let (_root, comb) = fixture();
    let mut req = search_request();
    req.graph.as_mut().unwrap().to = Some("concepts/d".parse().unwrap());
    let result = comb.index.search(&req).unwrap();
    assert_eq!(
        result
            .results
            .iter()
            .map(|h| h.slug.as_str())
            .collect::<Vec<_>>(),
        ["concepts/b", "concepts/d"]
    );
    let graph = result.graph.unwrap();
    assert!(graph.nodes.iter().any(|n| n.slug.as_str() == "concepts/c"));
    assert_eq!(graph.connection.unwrap().edges.len(), 2);
}

#[test]
fn graph_only_large_utf8_page_has_a_bounded_preview_before_long_query_matching() {
    let (_root, mut comb) = fixture();
    let text = "é ".repeat(300_000);
    put(&mut comb, "concepts/large", &text, false);
    put(
        &mut comb,
        "concepts/a",
        &relations(&["concepts/large"]),
        false,
    );
    let limit = crate::index::limits::SNIPPET_CHARS;
    let preview: String = comb
        .index
        .connection
        .query_row(
            crate::index::queries::search::HYDRATE_GRAPH,
            rusqlite::params!["concepts/large", crate::convert::usize_u32_capped(limit)],
            |row| row.get(2),
        )
        .unwrap();
    assert_eq!(preview, text.chars().take(limit).collect::<String>());
    let mut req = search_request();
    req.query = "a".repeat(crate::search::limits::QUERY_BYTES);
    let result = comb.index.search(&req).unwrap();
    assert_eq!(result.results.len(), 1);
    assert_eq!(result.results[0].matched_by, Route::Graph);
    assert_eq!(result.results[0].snippet, preview);
    assert!(result.results[0].highlights.is_empty());
    assert!(result.graph.unwrap().complete);
}
