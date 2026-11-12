use super::*;
use crate::values::{FindingKind, LinkKind};

#[test]
// O17/O18: classic traversal and union reports deliberately have separate contracts.
fn union_reports_deduplicate_classify_paginate_and_keep_classic_graph_ordinary_only() {
    let (_root, mut comb) = fixture();
    let a = "concepts/a".parse().unwrap();
    assert_eq!(comb.index.graph(&a, 2).unwrap(), std::slice::from_ref(&a));
    put(
        &mut comb,
        "concepts/a",
        "[[concepts/b]]\n\n## Relations\n- [[depends_on::concepts/b]]\n- [[depends_on::concepts/b]]\n- [[related_to::concepts/b]]\n- [[owned_by::concepts/absent]]\n- [[depends_on::concepts/c]]",
        false,
    );
    let report = comb.index.link_report(&a, false, None, 2).unwrap();
    assert!(report.complete);
    assert_eq!(report.references.len(), 2);
    assert_eq!(report.references[0].slug.as_str(), "concepts/absent");
    assert_eq!(report.references[0].page_unverified, None);
    let b = &report.references[1];
    assert_eq!(b.ordinary, [LinkKind::Slug]);
    assert_eq!(b.ordinary_source_page_unverified, Some(true));
    assert_eq!(b.typed.len(), 2);
    assert!(b.typed.iter().all(|t| t.unverified));
    let last = comb
        .index
        .link_report(&a, false, report.next_after.as_ref().map(Slug::as_str), 2)
        .unwrap();
    assert_eq!(last.references.len(), 1);
    assert!(last.next_after.is_none());
    let incoming = comb
        .index
        .link_report(&"concepts/c".parse().unwrap(), true, None, 1)
        .unwrap();
    assert_eq!(incoming.references[0].slug, a);
    assert!(incoming.references[0].ordinary.is_empty());
    assert_eq!(
        comb.index
            .graph(&a, 2)
            .unwrap()
            .iter()
            .map(Slug::as_str)
            .collect::<Vec<_>>(),
        ["concepts/a", "concepts/b"]
    );
}

#[test]
// O18: incomplete extraction qualifies zero-reference findings.
fn typed_only_references_suppress_orphans_and_incomplete_zero_is_qualified() {
    let (_root, mut comb) = fixture();
    put(&mut comb, "concepts/alone", "Standalone", false);
    let report = comb.doctor().unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.slug == "concepts/alone" && f.kind == FindingKind::Orphan)
    );
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.slug == "concepts/d" && f.kind == FindingKind::Orphan)
    );
    put(
        &mut comb,
        "concepts/bad",
        "## Relations\n- [[oops::concepts/alone]]",
        false,
    );
    let report = comb.doctor().unwrap();
    assert_eq!(report.graph.current_incomplete_sources, 1);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.slug == "concepts/alone"
                && f.kind == FindingKind::ReferenceCoverageIncomplete)
    );
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::Orphan)
    );
    let links = comb
        .index
        .link_report(&"concepts/alone".parse().unwrap(), true, None, 100)
        .unwrap();
    assert!(links.references.is_empty() && !links.complete);
}

#[test]
fn doctor_exposes_past_declarations_separately_and_reports_support_and_target_issues() {
    let (_root, mut comb) = fixture();
    let a = "concepts/a".parse().unwrap();
    comb.append(
        &a,
        crate::store::AppendEvidence {
            text: "## Relations\n- [[depends_on::concepts/past]]",
            source: "fixture",
            compiled: None,
            expected: None,
        },
        &write(&Actor::Human, "past", None).unwrap(),
    )
    .unwrap();
    put(
        &mut comb,
        "concepts/a",
        "## Relations\n- [[depends_on::concepts/missing]] [evidence:missing]",
        false,
    );
    let report = comb.doctor().unwrap();
    assert_eq!(report.graph.historical_reviewed, 1);
    assert_eq!(report.graph.past_occurrences.len(), 1);
    assert_eq!(
        report.graph.past_occurrences[0].target.as_str(),
        "concepts/past"
    );
    assert!(!report.graph.past_occurrences[0].unverified);
    assert!(
        report
            .graph
            .problems
            .iter()
            .any(|p| matches!(p.kind, graph::ProblemKind::UnresolvedEvidence))
    );
    assert!(report.graph.problems.iter().any(|p| matches!(
        p.kind,
        graph::ProblemKind::MissingTarget
    ) && p.location.zone
        == crate::values::Zone::Compiled));
    assert_eq!(
        comb.index
            .link_report(&a, false, None, 100)
            .unwrap()
            .references
            .len(),
        1
    );
    assert!(
        comb.index
            .link_report(&"concepts/past".parse().unwrap(), true, None, 100)
            .unwrap()
            .references
            .is_empty()
    );
}

#[test]
fn doctor_details_are_bounded_without_losing_omission_counts() {
    let (_root, mut comb) = fixture();
    let declarations = (0..256)
        .map(|i| format!("- [[depends_on::concepts/missing-{i}]] [evidence:missing-{i}]"))
        .collect::<Vec<_>>()
        .join("\n");
    put(
        &mut comb,
        "concepts/a",
        &format!("## Relations\n{declarations}"),
        false,
    );
    let report = comb.doctor().unwrap();
    assert_eq!(report.graph.problems.len(), 256);
    assert_eq!(report.graph.problems_omitted, 256);
    assert_eq!(report.graph.current_incomplete_sources, 0);
}
