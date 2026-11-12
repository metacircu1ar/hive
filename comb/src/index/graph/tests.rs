use super::*;
use crate::{
    Actor, Comb, Page,
    read::Snapshot,
    test_support::{TestComb, write},
};
mod acceptance;
mod coverage;
mod reports;
mod search;

#[test]
fn m5c_filtered_anchor_does_not_hide_missing_destination_and_cancel_stays_bounded() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    put(&mut comb, "concepts/a", "No relations.", true);
    let mut q = request("concepts/a");
    q.kind = Some("excluded-kind".into());
    q.to = Some("concepts/missing".parse().unwrap());
    assert!(query(&comb, &q).unwrap_err().is_not_found());
    let snapshot = Snapshot::pin(
        Index::reader(&comb.disk.index_path(), None).unwrap(),
        &comb.publication,
    )
    .unwrap();
    let cancel = Cancellation::default();
    cancel.cancel();
    let report = snapshot.index.graph_query(&q, cancel).unwrap();
    assert_eq!(report.incomplete_reasons, [Incomplete::Cancelled]);
    assert!(report.nodes.is_empty());
    drop(snapshot);
    q.to = Some(q.anchor.clone());
    let report = query(&comb, &q).unwrap();
    assert!(report.complete && report.nodes.is_empty() && report.connection.is_none());
}

fn put(comb: &mut Comb, slug: &str, summary: &str, human: bool) {
    let actor = if human {
        Actor::Human
    } else {
        crate::test_support::client("writer", "Writer")
    };
    let slug = slug.parse().unwrap();
    let prior = match comb.get(&slug) {
        Ok(page) => Some(page),
        Err(error) if error.is_not_found() => None,
        Err(error) => panic!("fixture get failed: {error}"),
    };
    let mut page = prior.as_ref().map_or_else(
        || Page::parse("---\ntitle: Fixture\n---\n").unwrap(),
        |p| p.page.clone(),
    );
    page.compiled = summary.into();
    comb.put_markdown(
        &slug,
        &page.render().unwrap(),
        prior.as_ref().map(|p| &p.hash),
        &write(&actor, &uuid::Uuid::new_v4().to_string(), None).unwrap(),
    )
    .unwrap();
}
fn query(comb: &Comb, request: &Query) -> Result<Report> {
    let snapshot = Snapshot::pin(
        Index::reader(&comb.disk.index_path(), None)?,
        &comb.publication,
    )?;
    snapshot.index.graph_query(request, Cancellation::default())
}
fn request(anchor: &str) -> Query {
    Query::new(
        anchor.parse().unwrap(),
        vec![RelationPredicate::DependsOn, RelationPredicate::RelatedTo],
    )
}
fn relations(targets: &[&str]) -> String {
    format!(
        "## Relations\n{}",
        targets
            .iter()
            .map(|s| format!("- [[depends_on::{s}]]"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}
fn fixture() -> (tempfile::TempDir, Comb) {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    put(
        &mut comb,
        "concepts/a",
        &relations(&["concepts/b", "concepts/c"]),
        false,
    );
    put(&mut comb, "concepts/b", &relations(&["concepts/d"]), false);
    put(&mut comb, "concepts/c", &relations(&["concepts/d"]), false);
    put(&mut comb, "concepts/d", "Target", false);
    (root, comb)
}

#[test]
fn default_unreviewed_neighbors_and_deterministic_shortest_connection() {
    let (_root, comb) = fixture();
    let mut q = request("concepts/a");
    let r = query(&comb, &q).unwrap();
    assert!(r.complete);
    assert_eq!(r.relations.len(), 4);
    assert!(r.relations.iter().all(|e| e.declaration.unverified));
    assert_eq!(r.work.nodes_visited, 4);
    q.to = Some("concepts/d".parse().unwrap());
    let r = query(&comb, &q).unwrap();
    let path = r.connection.unwrap();
    assert!(path.shortest && path.unverified);
    assert_eq!(
        path.nodes.iter().map(Slug::as_str).collect::<Vec<_>>(),
        ["concepts/a", "concepts/b", "concepts/d"]
    );
    assert_eq!(path.edges.len(), 2);
    q.reviewed_only = true;
    assert!(query(&comb, &q).unwrap().connection.is_none());
}

#[test]
// N06: global incomplete extraction, strict refusal, and scope exclusion.
fn current_coverage_is_global_and_partial_does_not_hide_valid_results() {
    let (_root, mut comb) = fixture();
    put(
        &mut comb,
        "concepts/typo",
        "## Relations\n- [[depends_on::Wrong]]",
        false,
    );
    let mut q = request("concepts/a");
    let r = query(&comb, &q).unwrap();
    assert_eq!(r.relations.len(), 4);
    assert!(!r.complete);
    assert_eq!(r.incomplete_reasons, [Incomplete::Extraction]);
    q.strict = true;
    assert_eq!(
        query(&comb, &q).unwrap_err().code,
        crate::ErrorCode::GraphIncomplete
    );
    q.strict = false;
    q.prefix = Some("concepts/a".into());
    let r = query(&comb, &q).unwrap();
    assert!(r.complete && r.relations.is_empty());
}

#[test]
// D17/D18: source provenance survives reverse traversal.
fn incoming_and_symmetric_queries_keep_actual_source_evidence() {
    let (_root, mut comb) = fixture();
    let mut q = request("concepts/d");
    q.direction = graph::Direction::Incoming;
    let r = query(&comb, &q).unwrap();
    assert_eq!(r.relations.len(), 4);
    assert_eq!(r.relations[0].source.as_str(), "concepts/b");
    assert_eq!(
        r.relations[0].declaration.text,
        "[[depends_on::concepts/d]]"
    );
    put(
        &mut comb,
        "concepts/e",
        "## Relations\n- [[related_to::concepts/d]]",
        true,
    );
    q.direction = graph::Direction::Outgoing;
    let r = query(&comb, &q).unwrap();
    assert_eq!(r.relations.len(), 1);
    assert_eq!(r.relations[0].source.as_str(), "concepts/e");
}

#[test]
fn cycle_cross_parallel_edges_and_depth_boundary_are_not_a_bfs_tree() {
    let (_root, mut comb) = fixture();
    put(
        &mut comb,
        "concepts/b",
        "## Relations\n- [[depends_on::concepts/a]]\n- [[related_to::concepts/a]]\n- [[depends_on::concepts/c]]\n- [[depends_on::concepts/d]]",
        false,
    );
    let mut q = request("concepts/a");
    let r = query(&comb, &q).unwrap();
    assert!(r.complete);
    assert_eq!(r.relations.len(), 7);
    q.depth = 0;
    let r = query(&comb, &q).unwrap();
    assert_eq!(r.nodes.len(), 1);
    assert!(r.relations.is_empty());
    q.depth = 1;
    let r = query(&comb, &q).unwrap();
    assert_eq!(r.relations.len(), 3); // a->b, a->c, symmetric b->a.
}

#[test]
// T10/N18: an excluded intermediate cannot bridge; a path cannot be half cited.
fn filters_cannot_be_crossed_and_budgets_never_return_half_a_path() {
    let (_root, mut comb) = fixture();
    put(
        &mut comb,
        "other/middle",
        &relations(&["concepts/d"]),
        false,
    );
    put(
        &mut comb,
        "concepts/a",
        &relations(&["other/middle"]),
        false,
    );
    let mut q = request("concepts/a");
    q.to = Some("concepts/d".parse().unwrap());
    q.prefix = Some("concepts/".into());
    assert!(query(&comb, &q).unwrap().connection.is_none());
    q.prefix = None;
    assert!(query(&comb, &q).unwrap().connection.is_some());
    q.max_evidence_bytes = 30;
    let r = query(&comb, &q).unwrap();
    assert!(!r.complete && r.connection.is_none());
    assert!(r.work.evidence_bytes <= 30);
    q.max_evidence_bytes = graph::default_evidence();
    q.max_nodes = 1;
    assert_eq!(
        query(&comb, &q).unwrap().incomplete_reasons,
        [Incomplete::Nodes]
    );
}

#[test]
// D11/D12/N11: supporting evidence has separate trust and cannot revive an edge.
fn current_edges_disappear_with_summary_removal_and_supplementary_support_is_independent() {
    let (_root, mut comb) = fixture();
    let slug = "concepts/a".parse().unwrap();
    comb.append(
        &slug,
        crate::store::AppendEvidence {
            text: "Old assertion",
            source: "fixture",
            compiled: None,
            expected: None,
        },
        &write(&Actor::Human, "append", None).unwrap(),
    )
    .unwrap();
    let document = comb.get(&slug).unwrap();
    let id = &document.page.timeline[0].metadata.authored.id;
    put(
        &mut comb,
        "concepts/a",
        &format!("## Relations\n- [[depends_on::concepts/b]] [evidence:{id}] [evidence:missing]"),
        false,
    );
    let q = request("concepts/a");
    let r = query(&comb, &q).unwrap();
    assert!(r.complete);
    let edge = &r.relations[0];
    assert!(!edge.support_complete);
    assert!(edge.declaration.unverified);
    assert_eq!(edge.unresolved_references, ["missing"]);
    assert!(!edge.support[0].unverified);
    put(&mut comb, "concepts/a", "Removed", false);
    assert!(query(&comb, &q).unwrap().relations.is_empty());
}

#[test]
fn unresolved_support_shares_the_global_evidence_budget() {
    let (_root, mut comb) = fixture();
    let oversized = "x".repeat(crate::relations::REFERENCE_BYTES);
    put(
        &mut comb,
        "concepts/a",
        &format!(
            "## Relations\n- [[depends_on::concepts/b]] [evidence:missing] [evidence:{oversized}]\n- [[depends_on::concepts/c]] [evidence:another-missing]"
        ),
        false,
    );
    let mut q = request("concepts/a");
    q.depth = 1;
    q.max_evidence_bytes = 64;
    let r = query(&comb, &q).unwrap();
    assert_eq!(r.relations.len(), 2);
    assert_eq!(r.incomplete_reasons, [Incomplete::EvidenceBytes]);
    assert_eq!(r.relations[0].unresolved_references, ["missing"]);
    assert!(r.relations[1].unresolved_references.is_empty());
    assert!(r.relations.iter().all(|e| !e.support_complete));
    assert_eq!(r.relations[0].omitted_references, 1);
    assert_eq!(r.relations[1].omitted_references, 1);
    let returned_bytes: usize = r
        .relations
        .iter()
        .map(|e| {
            e.declaration.text.len()
                + e.unresolved_references
                    .iter()
                    .map(String::len)
                    .sum::<usize>()
        })
        .sum();
    assert_eq!(r.work.evidence_bytes, returned_bytes);
    assert!(returned_bytes <= q.max_evidence_bytes);
}

#[test]
// N19/N20: cancellation is not corruption, and does not poison the next query.
fn cancellation_and_corruption_are_distinct_and_handler_does_not_leak() {
    let (_root, comb) = fixture();
    let snapshot = Snapshot::pin(
        Index::reader(&comb.disk.index_path(), None).unwrap(),
        &comb.publication,
    )
    .unwrap();
    let cancel = Cancellation::default();
    cancel.cancel();
    let q = request("concepts/a");
    let r = snapshot.index.graph_query(&q, cancel).unwrap();
    assert_eq!(r.incomplete_reasons, [Incomplete::Cancelled]);
    assert!(
        snapshot
            .index
            .graph_query(&q, Cancellation::default())
            .unwrap()
            .complete
    );
    drop(snapshot);
    comb.index
        .connection
        .execute("UPDATE relation_occurrences SET start=1", [])
        .unwrap();
    assert_eq!(
        query(&comb, &q).unwrap_err().code,
        crate::ErrorCode::IndexError
    );
}

#[test]
fn generations_pin_topology_and_source_spans_across_concurrent_publication() {
    let (_root, mut comb) = fixture();
    let q = request("concepts/a");
    let snapshot = Snapshot::pin(
        Index::reader(&comb.disk.index_path(), None).unwrap(),
        &comb.publication,
    )
    .unwrap();
    let prior = snapshot
        .index
        .graph_query(&q, Cancellation::default())
        .unwrap();
    put(
        &mut comb,
        "concepts/a",
        "## Relations\n- [[depends_on::concepts/d]]",
        false,
    );
    let pinned = snapshot
        .index
        .graph_query(&q, Cancellation::default())
        .unwrap();
    assert_eq!(pinned.generation, prior.generation);
    assert_eq!(
        pinned.relations[0].source_hash,
        prior.relations[0].source_hash
    );
    assert_eq!(pinned.relations.len(), 4);
    let latest = query(&comb, &q).unwrap();
    assert_eq!(latest.generation.identity, prior.generation.identity);
    assert!(latest.generation.revision > prior.generation.revision);
    assert_eq!(latest.relations.len(), 1);
    assert_eq!(
        latest.relations[0].declaration.text,
        "[[depends_on::concepts/d]]"
    );
}

#[test]
// N17: node/edge/match work bounds apply to the entire traversal.
fn edge_and_match_caps_are_global_and_missing_anchors_are_explicit() {
    let (_root, comb) = fixture();
    let mut q = request("concepts/a");
    q.max_edges = 2;
    let r = query(&comb, &q).unwrap();
    assert_eq!(r.work.edges_examined, 2);
    assert_eq!(r.incomplete_reasons, [Incomplete::Edges]);
    q.max_edges = graph::default_edges();
    q.limit = 1;
    let r = query(&comb, &q).unwrap();
    assert_eq!(r.relations.len(), 1);
    assert_eq!(r.incomplete_reasons, [Incomplete::Matches]);
    q.anchor = "concepts/missing".parse().unwrap();
    assert_eq!(
        query(&comb, &q).unwrap_err().code,
        crate::ErrorCode::NotFound
    );
}
