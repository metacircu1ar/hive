//! Frozen direct-query and path cases, beyond extraction-only assertions.
use super::*;

#[test]
fn direct_predicates_annotations_and_provenance_d01_d10_d19_d20() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    put(&mut comb, "concepts/b", "Target", true);
    for (id, predicate, marker, prefix, suffix) in [
        ("D01", "depends_on", "- ", "", ""),
        ("D02", "implements", "- ", "", ""),
        ("D03", "owned_by", "- ", "", ""),
        ("D04", "related_to", "- ", "", ""),
        ("D05", "depends_on", "- ", "", " — card payments"),
        ("D06", "depends_on", "- ", "", " no longer required"),
        ("D07", "depends_on", "-   ", "", ""),
        ("D08", "depends_on", "* ", "", ""),
        ("D08", "depends_on", "+ ", "", ""),
        ("D09", "depends_on", "- ", "Été 😀\n\n", " — café"),
        ("D10", "depends_on", "- ", "", " — first\n  second line"),
    ] {
        let token = format!("[[{predicate}::concepts/b]]");
        let summary = format!("{prefix}## Relations\n{marker}{token}{suffix}");
        put(&mut comb, "concepts/a", &summary, true);
        let mut q = Query::new(
            "concepts/a".parse().unwrap(),
            vec![RelationPredicate::from_token(predicate).unwrap()],
        );
        q.depth = 1;
        let r = query(&comb, &q).unwrap();
        assert!(r.complete, "{id}");
        assert_eq!(r.relations.len(), 1, "{id}");
        let edge = &r.relations[0];
        assert_eq!(edge.predicate.to_string(), predicate, "{id}");
        assert_eq!(edge.source.as_str(), "concepts/a", "{id}");
        assert_eq!(edge.target.as_str(), "concepts/b", "{id}");
        assert!(!edge.declaration.unverified, "{id}");
        assert_eq!(
            &summary[edge.declaration.start..edge.declaration.end],
            token,
            "{id}"
        );
        assert_eq!(edge.source_hash, comb.get(&q.anchor).unwrap().hash, "{id}");
        q.to = Some("concepts/b".parse().unwrap());
        let path = query(&comb, &q).unwrap().connection.unwrap();
        assert!(path.shortest && !path.unverified, "D19");
        assert_eq!(path.edges.len(), 1, "D19");
    }
    put(&mut comb, "concepts/a", &relations(&["concepts/b"]), false);
    let mut q = request("concepts/a");
    assert!(
        query(&comb, &q).unwrap().relations[0]
            .declaration
            .unverified,
        "D20"
    );
    q.reviewed_only = true;
    for strict in [false, true] {
        q.strict = strict;
        let r = query(&comb, &q).unwrap();
        assert!(r.complete && r.relations.is_empty(), "D20");
    }
}

fn chain() -> (tempfile::TempDir, Comb) {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    put(&mut comb, "concepts/a", &relations(&["concepts/b"]), true);
    put(&mut comb, "concepts/b", &relations(&["concepts/c"]), true);
    put(&mut comb, "concepts/c", "Target", true);
    (root, comb)
}

#[test]
fn chain_direction_t01_t04() {
    let (_root, comb) = chain();
    let mut q = request("concepts/a");
    let r = query(&comb, &q).unwrap();
    assert!(
        r.complete && r.relations.len() == 2 && r.nodes.len() == 3,
        "T01"
    );
    q.to = Some("concepts/c".parse().unwrap());
    assert_eq!(
        query(&comb, &q)
            .unwrap()
            .connection
            .unwrap()
            .nodes
            .iter()
            .map(Slug::as_str)
            .collect::<Vec<_>>(),
        ["concepts/a", "concepts/b", "concepts/c"],
        "T01"
    );
    q.to = None;
    q.depth = 1;
    let r = query(&comb, &q).unwrap();
    assert!(r.complete && r.relations.len() == 1, "T02");
    q = request("concepts/c");
    let r = query(&comb, &q).unwrap();
    assert!(
        r.complete && r.nodes.len() == 1 && r.relations.is_empty(),
        "T04"
    );
    q.direction = graph::Direction::Incoming;
    q.to = Some("concepts/a".parse().unwrap());
    let r = query(&comb, &q).unwrap();
    assert!(r.complete, "T03");
    let path = r.connection.unwrap();
    assert_eq!(
        path.edges
            .iter()
            .map(|e| e.source.as_str())
            .collect::<Vec<_>>(),
        ["concepts/b", "concepts/a"],
        "T03"
    );
}

#[test]
fn predicate_selection_and_shorter_path_t05_t07_t19() {
    let (_root, mut comb) = chain();
    put(
        &mut comb,
        "concepts/b",
        "## Relations\n- [[owned_by::concepts/c]]",
        true,
    );
    let mut q = request("concepts/a");
    q.predicates = vec![RelationPredicate::DependsOn, RelationPredicate::OwnedBy];
    let r = query(&comb, &q).unwrap();
    assert!(r.complete && r.relations.len() == 2, "T05");
    q.predicates = vec![RelationPredicate::DependsOn];
    let r = query(&comb, &q).unwrap();
    assert!(
        r.complete && r.relations.len() == 1 && r.nodes.len() == 2,
        "T06"
    );
    put(
        &mut comb,
        "concepts/a",
        "## Relations\n- [[related_to::concepts/b]]",
        true,
    );
    put(&mut comb, "concepts/b", "No outgoing declarations", true);
    put(&mut comb, "concepts/c", &relations(&["concepts/b"]), true);
    q = request("concepts/a");
    q.direction = graph::Direction::Both;
    q.to = Some("concepts/c".parse().unwrap());
    let r = query(&comb, &q).unwrap();
    assert!(r.complete, "T07");
    let path = r.connection.unwrap();
    assert_eq!(
        path.edges
            .iter()
            .map(|e| e.source.as_str())
            .collect::<Vec<_>>(),
        ["concepts/a", "concepts/c"],
        "T07"
    );
    put(
        &mut comb,
        "concepts/a",
        &relations(&["concepts/b", "concepts/c"]),
        true,
    );
    put(&mut comb, "concepts/b", &relations(&["concepts/c"]), true);
    q.direction = graph::Direction::Outgoing;
    let r = query(&comb, &q).unwrap();
    assert!(r.complete && r.connection.as_ref().unwrap().shortest, "T19");
    assert_eq!(r.connection.unwrap().edges.len(), 1, "T19");
}

#[test]
fn prefix_filters_destination_without_hiding_the_intermediate_t10() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    put(&mut comb, "concepts/a", &relations(&["concepts/b"]), true);
    put(&mut comb, "concepts/b", &relations(&["reports/c"]), true);
    put(&mut comb, "reports/c", "Target", true);
    let mut q = request("concepts/a");
    q.prefix = Some("concepts/".into());
    let r = query(&comb, &q).unwrap();
    assert!(r.complete && r.relations.len() == 1 && r.nodes.len() == 2);
    assert_eq!(r.relations[0].target.as_str(), "concepts/b");
}

#[expect(
    clippy::too_many_arguments,
    reason = "Graph filter fixture varies slug, summary, kind, tag and timestamp independently"
)]
fn indexed_page(comb: &mut Comb, slug: &str, summary: &str, kind: &str, tag: &str, at: &str) {
    let raw = format!(
        "---\ntitle: Fixture\ntype: {kind}\ntags: [{tag}]\nupdated_at: '{at}'\n---\n{summary}"
    );
    comb.index
        .upsert(&slug.parse().unwrap(), &raw, &Page::parse(&raw).unwrap())
        .unwrap();
}

#[test]
fn each_filter_blocks_intermediates_t08_t09_t11_t12() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    // Index-level fixture keeps deterministic update timestamps, as ordinary
    // search filter tests do. This exercises the real projection and SQL scope.
    for (id, kind, tag, at) in [
        ("control", "concept", "focus", "2026-01-02T00:00:00Z"),
        ("T08", "note", "focus", "2026-01-02T00:00:00Z"),
        ("T09", "concept", "other", "2026-01-02T00:00:00Z"),
        ("T11", "concept", "focus", "2026-01-01T00:00:00Z"),
        ("T12", "concept", "focus", "2026-01-04T00:00:00Z"),
    ] {
        for (slug, summary, kind, tag, at) in [
            (
                "concepts/a",
                relations(&["concepts/b"]),
                "concept",
                "focus",
                "2026-01-02T02:00:00+02:00",
            ),
            ("concepts/b", relations(&["concepts/c"]), kind, tag, at),
            (
                "concepts/c",
                "Target".into(),
                "concept",
                "focus",
                "2026-01-03T00:00:00Z",
            ),
        ] {
            indexed_page(&mut comb, slug, &summary, kind, tag, at);
        }
        let mut q = request("concepts/a");
        q.to = Some("concepts/c".parse().unwrap());
        assert!(
            query(&comb, &q).unwrap().connection.is_some(),
            "{id} control"
        );
        q.kind = Some("concept".into());
        q.tag = Some("focus".into());
        q.after = Some("2026-01-02T00:00:00Z".parse().unwrap());
        q.before = Some("2026-01-03T00:00:00Z".parse().unwrap());
        let r = query(&comb, &q).unwrap();
        assert!(r.complete, "{id}");
        if id == "control" {
            assert_eq!(
                r.connection.unwrap().nodes.len(),
                3,
                "filtered positive control"
            );
        } else {
            assert!(r.connection.is_none() && r.nodes.len() == 1, "{id}");
        }
    }
}

#[test]
fn filtered_endpoints_have_eligible_intermediates_t13_t14() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    for (slug, summary, at) in [
        (
            "concepts/a",
            relations(&["concepts/b"]),
            "2026-01-02T00:00:00Z",
        ),
        (
            "concepts/b",
            relations(&["concepts/c"]),
            "2026-01-02T00:00:00Z",
        ),
        ("concepts/c", "Target".into(), "2026-01-03T00:00:00Z"),
    ] {
        indexed_page(&mut comb, slug, &summary, "concept", "focus", at);
    }
    let mut q = request("concepts/a");
    q.prefix = Some("concepts/b".into());
    let r = query(&comb, &q).unwrap();
    assert!(
        r.complete && r.nodes.is_empty() && r.relations.is_empty(),
        "T13"
    );
    q.prefix = None;
    q.to = Some("concepts/c".parse().unwrap());
    q.before = Some("2026-01-03T00:00:00Z".parse().unwrap());
    let r = query(&comb, &q).unwrap();
    assert!(
        r.complete && r.connection.is_some(),
        "T14 inclusive destination control"
    );
    q.before = Some("2026-01-02T23:00:00Z".parse().unwrap());
    let r = query(&comb, &q).unwrap();
    assert!(
        r.complete && r.connection.is_none() && r.nodes.is_empty(),
        "T14 only destination excluded"
    );
    q.to = None;
    let r = query(&comb, &q).unwrap();
    assert_eq!(
        r.nodes
            .iter()
            .map(|node| node.slug.as_str())
            .collect::<Vec<_>>(),
        ["concepts/a", "concepts/b"],
        "T14 anchor and intermediate remain eligible"
    );
}

#[test]
fn equal_paths_remain_stable_after_rebuild_t18() {
    let (root, comb) = fixture();
    let mut q = request("concepts/a");
    q.to = Some("concepts/d".parse().unwrap());
    let before = query(&comb, &q).unwrap().connection.unwrap();
    assert_eq!(before.nodes[1].as_str(), "concepts/b");
    drop(comb);
    let comb = Comb::rebuild(root.path()).unwrap();
    let after = query(&comb, &q).unwrap().connection.unwrap();
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(before).unwrap()
    );
}

#[test]
fn duplicates_parallel_cycle_and_boundary_edges_d13_d14_d16_t15_t17_t20() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    put(
        &mut comb,
        "concepts/a",
        "## Relations\n- [[depends_on::concepts/b]]\n- [[depends_on::concepts/b]]\n- [[implements::concepts/b]]\n- [[depends_on::concepts/c]]",
        true,
    );
    put(&mut comb, "concepts/b", &relations(&["concepts/c"]), true);
    put(&mut comb, "concepts/c", &relations(&["concepts/a"]), true);
    let mut q = request("concepts/a");
    q.predicates.push(RelationPredicate::Implements);
    q.depth = 0;
    let r = query(&comb, &q).unwrap();
    assert!(
        r.complete && r.relations.is_empty() && r.nodes.len() == 1,
        "D16"
    );
    q.depth = 1;
    let r = query(&comb, &q).unwrap();
    assert!(
        r.complete && r.relations.len() == 3 && r.nodes.len() == 3,
        "D13/D14/T17/T20"
    );
    assert!(r.relations.iter().all(|e| e.source == q.anchor), "T20");
    let occurrences: u32 = comb.index.connection.query_row(
        "SELECT count(*) FROM relation_occurrences WHERE source='concepts/a' AND target='concepts/b' AND predicate='depends_on'", [], |r| r.get(0)).unwrap();
    assert_eq!(occurrences, 2, "D13 stored locations");
    q.depth = 3;
    let r = query(&comb, &q).unwrap();
    assert!(
        r.complete && r.relations.len() == 5 && r.nodes.len() == 3,
        "T15/T16/T20"
    );
    assert_eq!(
        r.relations
            .iter()
            .filter(|e| e.source.as_str() == "concepts/c")
            .count(),
        1,
        "T15"
    );
    assert_eq!(
        r.relations
            .iter()
            .filter(|e| e.source.as_str() == "concepts/b")
            .count(),
        1,
        "T16/T20"
    );
}

#[test]
fn past_declarations_and_caps_never_resurrect_current_edges_d15_n07_n08_n11() {
    let (_root, mut comb) = chain();
    let slug = "concepts/a".parse().unwrap();
    let past = "## Relations\n- [[owned_by::concepts/c]]";
    comb.append(
        &slug,
        crate::store::AppendEvidence {
            text: past,
            source: "fixture",
            compiled: None,
            expected: None,
        },
        &write(&Actor::Human, "past", None).unwrap(),
    )
    .unwrap();
    let mut q = request("concepts/a");
    q.depth = 1;
    q.predicates.push(RelationPredicate::OwnedBy);
    let r = query(&comb, &q).unwrap();
    assert!(r.complete && r.relations.len() == 1, "D15/N11");
    assert_eq!(r.relations[0].target.as_str(), "concepts/b", "D15");
    let many = format!(
        "## Relations\n{}",
        "- [[owned_by::concepts/c]]\n".repeat(257)
    );
    comb.append(
        &slug,
        crate::store::AppendEvidence {
            text: &many,
            source: "fixture",
            compiled: None,
            expected: None,
        },
        &write(&Actor::Human, "past-cap", None).unwrap(),
    )
    .unwrap();
    let r = query(&comb, &q).unwrap();
    assert!(r.complete && r.relations.len() == 1, "N08");
    assert_eq!(
        r.coverage.totals.unwrap().historical_incomplete_sources,
        1,
        "N08"
    );
    put(&mut comb, "concepts/a", &many, true);
    let r = query(&comb, &q).unwrap();
    assert!(
        !r.complete && r.incomplete_reasons == [Incomplete::Extraction],
        "N07"
    );
    assert_eq!(r.relations.len(), 1, "N07 duplicates share one edge");
    let before = comb.get(&slug).unwrap().page.timeline;
    put(&mut comb, "concepts/a", "Retracted", true);
    let r = query(&comb, &q).unwrap();
    assert!(r.complete && r.relations.is_empty(), "N11");
    assert_eq!(comb.get(&slug).unwrap().page.timeline, before, "N11");
}
