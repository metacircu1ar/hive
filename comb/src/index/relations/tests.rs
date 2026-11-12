use super::*;
use crate::{
    Actor, Comb,
    test_support::{TestComb, write},
};

fn rows(index: &Index) -> Vec<(String, String, String, bool, String)> {
    index
        .connection
        .prepare(
            "SELECT source,zone,target,verified,content_hash FROM relation_occurrences \
         ORDER BY source,ordinal",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn markdown(target: &str) -> String {
    format!("---\ntitle: Fixture\n---\n## Relations\n\n- [[depends_on::{target}]] — annotation\n")
}

#[test]
fn incomplete_support_survives_index_rebuild_without_losing_a_valid_edge() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let slug = "concepts/a".parse().unwrap();
    let raw = format!("{} [evidence:unfinished", markdown("concepts/b").trim_end());
    comb.put_markdown(
        &slug,
        &raw,
        None,
        &write(&Actor::Human, "create", None).unwrap(),
    )
    .unwrap();
    let state = |index: &Index| {
        index
            .connection
            .query_row(
                "SELECT r.references_complete,c.current_incomplete FROM relation_occurrences r \
             JOIN relation_coverage c ON c.source=r.source",
                [],
                |r| Ok((r.get::<_, bool>(0)?, r.get::<_, bool>(1)?)),
            )
            .unwrap()
    };
    assert_eq!(state(&comb.index), (false, false));
    assert_eq!(rows(&comb.index)[0].2, "concepts/b");
    drop(comb);
    let comb = Comb::rebuild(root.path()).unwrap();
    assert_eq!(state(&comb.index), (false, false));
    assert_eq!(rows(&comb.index)[0].2, "concepts/b");
}

#[test]
fn page_updates_trust_and_rebuild_project_identical_current_and_historical_rows_n09_n11_n16() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let slug: Slug = "concepts/a".parse().unwrap();
    let actor = crate::test_support::client("writer", "Writer");
    let first = comb
        .put_markdown(
            &slug,
            &markdown("concepts/b"),
            None,
            &write(&actor, "create", None).unwrap(),
        )
        .unwrap();
    assert_eq!(rows(&comb.index)[0].4, first.hash.as_str());
    assert!(!rows(&comb.index)[0].3);
    let reviewed = write(&Actor::Human, "promote", None)
        .unwrap()
        .review_for_promotion(false)
        .unwrap();
    comb.promote(&slug, &first.hash, &reviewed).unwrap();
    assert!(rows(&comb.index)[0].3);
    let document = comb.get(&slug).unwrap();
    comb.append(
        &slug,
        crate::store::AppendEvidence {
            text: "## Relations\n- [[owned_by::concepts/c]]",
            source: "fixture",
            compiled: None,
            expected: None,
        },
        &write(&actor, "append", None).unwrap(),
    )
    .unwrap();
    assert_eq!(rows(&comb.index).len(), 2);
    assert!(rows(&comb.index)[0].3);
    assert!(!rows(&comb.index)[1].3);
    let snapshot = rows(&comb.index);
    let raw = std::fs::read(root.path().join("concepts/a.md")).unwrap();
    drop(comb);
    let mut comb = Comb::rebuild(root.path()).unwrap();
    assert_eq!(rows(&comb.index), snapshot);
    assert_eq!(
        std::fs::read(root.path().join("concepts/a.md")).unwrap(),
        raw
    );
    let current = comb.get(&slug).unwrap();
    let mut page = current.page.clone();
    page.compiled = "No current relations.".into();
    comb.put_markdown(
        &slug,
        &page.render().unwrap(),
        Some(&current.hash),
        &write(&actor, "remove-summary", None).unwrap(),
    )
    .unwrap();
    let left = rows(&comb.index);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].1, "timeline");
    assert_ne!(comb.get(&slug).unwrap().hash, document.hash);
}

#[test]
fn source_deletion_quarantine_and_recreation_replace_projection_n12_n14() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let slug: Slug = "concepts/a".parse().unwrap();
    let first = comb
        .put_markdown(
            &slug,
            &markdown("concepts/b"),
            None,
            &write(&Actor::Human, "create", None).unwrap(),
        )
        .unwrap();
    comb.index.quarantine(&slug, "invalid_page").unwrap();
    assert!(rows(&comb.index).is_empty());
    comb.sync().unwrap();
    assert_eq!(rows(&comb.index).len(), 1);
    comb.remove(
        &slug,
        &first.hash,
        &write(&Actor::Human, "remove", None).unwrap(),
    )
    .unwrap();
    assert!(rows(&comb.index).is_empty());
    comb.put_markdown(
        &slug,
        &markdown("concepts/d"),
        None,
        &write(&Actor::Human, "recreate", None).unwrap(),
    )
    .unwrap();
    assert_eq!(rows(&comb.index)[0].2, "concepts/d");
}

#[test]
fn failed_relation_insert_rolls_back_page_links_and_generation() {
    let dir = tempfile::tempdir().unwrap();
    let mut index = Index::open(&dir.path().canonicalize().unwrap().join("index.sqlite")).unwrap();
    let slug: Slug = "concepts/a".parse().unwrap();
    let raw = format!(
        "{}\nOrdinary [[concepts/ordinary-old]]",
        markdown("concepts/b")
    );
    index
        .upsert(&slug, &raw, &Page::parse(&raw).unwrap())
        .unwrap();
    let prior = rows(&index);
    let generation = || {
        index
            .connection
            .query_row("SELECT revision FROM index_generation", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap()
    };
    let before = generation();
    index
        .connection
        .execute_batch(
            "CREATE TRIGGER refuse_projection BEFORE INSERT ON relation_coverage \
         BEGIN SELECT RAISE(ABORT,'injected projection failure'); END;",
        )
        .unwrap();
    let changed = format!(
        "{}\nOrdinary [[concepts/ordinary-new]]",
        markdown("concepts/c")
    );
    assert!(
        index
            .upsert(&slug, &changed, &Page::parse(&changed).unwrap())
            .is_err()
    );
    assert_eq!(rows(&index), prior);
    assert_eq!(
        index.links(&slug, false).unwrap(),
        ["concepts/ordinary-old".parse::<Slug>().unwrap()]
    );
    assert_eq!(
        index.content_hash(&slug).unwrap().unwrap(),
        ContentHash::digest(&raw)
    );
    assert_eq!(
        index
            .connection
            .query_row("SELECT revision FROM index_generation", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        before
    );
}

#[test]
fn malformed_projection_is_explicit_and_does_not_reject_page_n03_n04() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let slug: Slug = "concepts/a".parse().unwrap();
    let raw = "---\ntitle: Fixture\n---\n## Relations\n- [[unknown::concepts/b]]";
    comb.put_markdown(
        &slug,
        raw,
        None,
        &write(&Actor::Human, "create", None).unwrap(),
    )
    .unwrap();
    assert!(comb.get(&slug).unwrap().page.compiled.contains("unknown::"));
    assert!(rows(&comb.index).is_empty());
    let incomplete: bool = comb
        .index
        .connection
        .query_row(
            "SELECT current_incomplete FROM relation_coverage",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(incomplete);
}

#[test]
fn target_delete_and_recreate_resolve_again_without_rewriting_assertion_n13() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let source = "concepts/a".parse().unwrap();
    let target = "concepts/b".parse().unwrap();
    comb.put_markdown(
        &source,
        &markdown("concepts/b"),
        None,
        &write(&Actor::Human, "source", None).unwrap(),
    )
    .unwrap();
    let resolvable = |comb: &Comb| {
        comb.index
            .connection
            .query_row(
                "SELECT count(*) FROM relation_occurrences r JOIN pages p ON p.slug=r.target",
                [],
                |r| r.get::<_, u32>(0),
            )
            .unwrap()
    };
    assert_eq!(resolvable(&comb), 0);
    let created = comb
        .put_markdown(
            &target,
            "---\ntitle: Target\n---\nFirst",
            None,
            &write(&Actor::Human, "target", None).unwrap(),
        )
        .unwrap();
    assert_eq!(resolvable(&comb), 1);
    let saved = rows(&comb.index);
    comb.remove(
        &target,
        &created.hash,
        &write(&Actor::Human, "delete", None).unwrap(),
    )
    .unwrap();
    assert_eq!(resolvable(&comb), 0);
    comb.put_markdown(
        &target,
        "---\ntitle: Target\n---\nReplacement",
        None,
        &write(&Actor::Human, "reuse", None).unwrap(),
    )
    .unwrap();
    assert_eq!(resolvable(&comb), 1);
    assert_eq!(rows(&comb.index), saved);
}

#[test]
fn external_edit_import_and_consolidation_use_the_same_projection_n14_n15() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let source = tempfile::tempdir().unwrap();
    std::fs::write(
        source.path().join("a.md"),
        "## Relations\n- [[depends_on::concepts/b]]",
    )
    .unwrap();
    comb.import(source.path(), Some(&"concepts".parse().unwrap()))
        .unwrap();
    let slug: Slug = "concepts/a".parse().unwrap();
    assert_eq!(rows(&comb.index)[0].2, "concepts/b");
    std::fs::write(root.path().join("concepts/a.md"), markdown("concepts/c")).unwrap();
    comb.sync().unwrap();
    assert_eq!(rows(&comb.index)[0].2, "concepts/c");
    assert!(!rows(&comb.index)[0].3);
    let actor = crate::test_support::client("curator", "Curator");
    let note = comb
        .capture(
            "New relation measurement",
            &write(&actor, "capture", Some("graph")).unwrap(),
        )
        .unwrap();
    let request = crate::store::Consolidate {
        note: note.slug,
        note_hash: note.hash,
        target: slug.clone(),
        target_hash: comb.get(&slug).unwrap().hash,
        summary: "## Relations\n- [[implements::concepts/d]] [evidence:new]".into(),
        recopy: false,
    };
    comb.consolidate(
        &request,
        &write(&actor, "consolidate", Some("graph")).unwrap(),
    )
    .unwrap();
    let saved = rows(&comb.index);
    assert_eq!(saved[0].2, "concepts/d");
    assert!(!saved[0].3);
    drop(comb);
    assert_eq!(rows(&Comb::rebuild(root.path()).unwrap().index), saved);
}

#[test]
fn durable_page_with_failed_projection_recovers_on_restart_n15() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    comb.index
        .connection
        .execute_batch(
            "CREATE TRIGGER refuse_projection BEFORE INSERT ON relation_coverage \
         BEGIN SELECT RAISE(ABORT,'injected projection failure'); END;",
        )
        .unwrap();
    let slug: Slug = "concepts/a".parse().unwrap();
    let failure = comb
        .put_markdown(
            &slug,
            &markdown("concepts/b"),
            None,
            &write(&Actor::Human, "create", None).unwrap(),
        )
        .unwrap_err();
    assert!(failure.file_committed);
    assert!(rows(&comb.index).is_empty());
    comb.index
        .connection
        .execute_batch("DROP TRIGGER refuse_projection")
        .unwrap();
    drop(comb);
    let comb = Comb::open(root.path()).unwrap();
    assert_eq!(rows(&comb.index)[0].2, "concepts/b");
    assert_eq!(
        rows(&comb.index)[0].4,
        comb.get(&slug).unwrap().hash.as_str()
    );
}
