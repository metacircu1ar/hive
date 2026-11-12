use super::*;
use crate::{
    Actor,
    page::{Entry, EntryAuthored, EntryMeta},
};

const TOKEN: &str = "[[depends_on::concepts/b]]";
const ID: &str = "00000000-0000-4000-8000-000000000001";

fn page(body: &str) -> Page {
    Page::parse(&format!("---\ntitle: Fixture\n---\n{body}\n")).unwrap()
}

fn timeline(page: &mut Page, body: &str, actor: &Actor) {
    page.timeline.push(Entry {
        metadata: EntryMeta::created_by(
            EntryAuthored {
                id: ID.parse().unwrap(),
                source: "fixture".into(),
            },
            actor,
            None,
        ),
        text: body.into(),
    });
}

#[test]
fn direct_predicates_annotations_and_byte_coordinates_d01_d10() {
    for predicate in ["depends_on", "implements", "owned_by", "related_to"] {
        for bullet in ['-', '*', '+'] {
            for annotation in [
                "",
                " — card payments",
                " no longer required",
                " αβ\n  continued",
            ] {
                let token = format!("[[{predicate}::concepts/b]]");
                let mut page = page(&format!(
                    "日本語\n\n## Relations\n\n{bullet}   {token}{annotation}"
                ));
                page.promote_summary(Actor::Human).unwrap();
                let result = extract(&page);
                assert!(!result.current_incomplete);
                assert_eq!(result.occurrences.len(), 1);
                let edge = &result.occurrences[0];
                assert_eq!(&page.compiled[edge.start..edge.end], token);
                assert_eq!(edge.start, page.compiled.find(&token).unwrap());
                assert_eq!(edge.predicate.as_str(), predicate);
                assert!(edge.verified);
            }
        }
    }
}

#[test]
fn evidence_annotations_ignore_code_and_escaped_examples_d11_d12() {
    let page = page(&format!(
        "## Relations\n\n- {TOKEN} [evidence:{ID}] [evidence:missing] \
         `[evidence:code]` \\[evidence:escaped]"
    ));
    let result = extract(&page);
    assert_eq!(result.occurrences[0].references, [ID, "missing"]);
    // Resolution is a query operation; absent support does not invalidate extraction.
    assert!(!result.current_incomplete);
    assert!(result.occurrences[0].references_complete);
}

#[test]
fn malformed_reference_runs_have_bounded_support_and_keep_the_declaration() {
    for annotation in [
        format!("{}]", "[evidence:".repeat(50_000)),
        "[evidence:".repeat(50_000),
        format!("[evidence:{}]", "x".repeat(REFERENCE_BYTES + 1)),
        "[evidence:outer[evidence:inner]".into(),
    ] {
        let result = extract(&page(&format!("## Relations\n- {TOKEN} {annotation}")));
        assert_eq!(result.occurrences.len(), 1);
        assert!(!result.current_incomplete);
        let edge = &result.occurrences[0];
        assert!(!edge.references_complete);
        assert!(edge.references.is_empty());
    }
}

#[test]
fn reference_count_is_bounded_and_duplicate_references_do_not_spend_the_limit() {
    use std::fmt::Write as _;
    let mut annotation = String::new();
    for i in 0..=REFERENCE_LIMIT {
        write!(annotation, "[evidence:missing-{i}] ").unwrap();
    }
    let result = extract(&page(&format!("## Relations\n- {TOKEN} {annotation}")));
    assert!(!result.current_incomplete);
    assert_eq!(result.occurrences[0].references.len(), REFERENCE_LIMIT);
    assert!(!result.occurrences[0].references_complete);
    let duplicates = format!("[evidence:{ID}] ").repeat(REFERENCE_LIMIT + 1);
    let result = extract(&page(&format!("## Relations\n- {TOKEN} {duplicates}")));
    assert_eq!(result.occurrences[0].references, [ID]);
    assert!(result.occurrences[0].references_complete);
}

#[test]
fn subsequent_annotation_tokens_do_not_assert_and_duplicates_keep_locations_d13_d15() {
    let mut page = page(&format!(
        "## Relations\n\n- {TOKEN} [[owned_by::concepts/c]]\n- {TOKEN}"
    ));
    timeline(
        &mut page,
        "## Relations\n\n- [[depends_on::concepts/c]]",
        &Actor::Human,
    );
    let result = extract(&page);
    assert_eq!(result.occurrences.len(), 3);
    assert_eq!(result.occurrences[0].target.as_str(), "concepts/b");
    assert_eq!(result.occurrences[1].target.as_str(), "concepts/b");
    assert_ne!(result.occurrences[0].start, result.occurrences[1].start);
    assert_eq!(result.occurrences[2].zone, Zone::Timeline);
    assert_eq!(result.occurrences[2].entry.as_ref().unwrap().as_str(), ID);
    assert!(result.occurrences[2].verified);
}

#[test]
fn ignored_markdown_shapes_are_never_assertions_n01_n02_n05() {
    for body in [
        format!("## Relations\n\n```text\n- {TOKEN}\n```"),
        format!("## Relations\n\n- `{TOKEN}`"),
        format!("## Relations\n\n> - {TOKEN}"),
        format!("## Relations\n\n<div>\n- {TOKEN}\n</div>"),
        format!("## Relations\n\n- \\{TOKEN}"),
        format!("## Relations\n\n- Parent\n  - {TOKEN}"),
        format!("## Relations\n\n- {TOKEN}\n\n  Second paragraph"),
        format!("## Relations\n\n1. {TOKEN}"),
        format!("## Relations\n\n- we no longer {TOKEN}"),
        format!("## Other\n\n- {TOKEN}\n\n## Relations\n"),
        format!("## Relations\n\n### Other\n\n- {TOKEN}"),
    ] {
        assert!(extract(&page(&body)).occurrences.is_empty(), "{body}");
    }
    assert!(!extract(&page("## Relations")).current_incomplete);
}

#[test]
fn deep_blockquote_lazy_continuations_do_not_multiply_extraction_work() {
    let quoted = format!(
        "{}a\n{}\n\n## Relations\n- {TOKEN}",
        ">".repeat(500_000),
        "b\n".repeat(250_000)
    );
    assert!(quoted.len() < crate::page::PAGE_BYTES);
    let page = page(&quoted);
    let start = std::time::Instant::now();
    let result = extract(&page);
    assert_eq!(result.occurrences.len(), 1);
    assert!(!result.current_incomplete);
    // A generous complexity guard, not a performance target: the prior per-text
    // stack scan performed about 125 billion comparisons for this admitted page.
    assert!(start.elapsed() < std::time::Duration::from_secs(30));
}

#[test]
fn duplicate_sections_discard_the_whole_source_and_malformed_tokens_are_diagnosed_n03_n04() {
    let result = extract(&page(&format!(
        "## Relations\n- {TOKEN}\n\n## Relations\n- {TOKEN}"
    )));
    assert!(result.current_incomplete);
    assert!(result.occurrences.is_empty());
    assert_eq!(result.diagnostics[0].issue, Issue::DuplicateSection);
    for (token, issue) in [
        ("[[supports::concepts/b]]", Issue::UnknownPredicate),
        ("[[depends_on::Concepts/b]]", Issue::InvalidTarget),
        ("[[depends_on::concepts/b#part]]", Issue::InvalidTarget),
        ("[[depends_on::concepts/b|Label]]", Issue::InvalidTarget),
        ("[[depends_on::concepts/b", Issue::MalformedToken),
    ] {
        let result = extract(&page(&format!("## Relations\n- {token}")));
        assert!(result.current_incomplete, "{token}");
        assert_eq!(result.diagnostics[0].issue, issue);
    }
}

#[test]
fn current_and_historical_budgets_are_independent_n07_n08() {
    let declarations = |n: usize| format!("## Relations\n{}", format!("- {TOKEN}\n").repeat(n));
    let mut page = page(&declarations(1));
    timeline(&mut page, &declarations(257), &Actor::Human);
    let result = extract(&page);
    assert!(!result.current_incomplete);
    assert!(result.historical_incomplete);
    assert_eq!(result.occurrences.len(), 257);
    page.compiled = declarations(257);
    let result = extract(&page);
    assert!(result.current_incomplete && result.historical_incomplete);
    assert_eq!(result.occurrences.len(), 512);
}

#[test]
fn trust_tracks_the_existing_summary_and_timeline_rules_d20_n09_n10() {
    let mut page = page(&format!("## Relations\n- {TOKEN}"));
    let client = crate::test_support::client("writer", "Writer");
    timeline(&mut page, &format!("## Relations\n- {TOKEN}"), &client);
    assert!(extract(&page).occurrences.iter().all(|edge| !edge.verified));
    page.promote(Actor::Human).unwrap();
    assert!(extract(&page).occurrences.iter().all(|edge| edge.verified));
    page.compiled.push_str("\n\nChanged summary");
    let result = extract(&page);
    assert!(!result.occurrences[0].verified);
    assert!(result.occurrences[1].verified);
    assert_eq!(result.occurrences[1].zone, Zone::Timeline);
}
