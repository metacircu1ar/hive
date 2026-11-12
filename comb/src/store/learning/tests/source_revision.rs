//! Source revision regressions.
use super::*;

#[test]
fn rewritten_source_or_missing_target_history_requires_explicit_recorded_recopy() {
    for source in [false, true] {
        let (directory, mut comb, note) = setup();
        let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
        comb.consolidate(
            &request(&comb, &note),
            &fixture_write(&actor, "first", Some("curation-1")).unwrap(),
        )
        .unwrap();
        hand_edit(
            directory.path(),
            &mut comb,
            if source { &note } else { "concepts/writer" },
            |page| {
                if source {
                    page.timeline[0].text = "Rewritten historical measurement".into();
                } else {
                    page.timeline.clear();
                }
            },
        );
        let next = request(&comb, &note);
        assert_eq!(
            comb.consolidate(
                &next,
                &fixture_write(&actor, "refused", Some("curation-1")).unwrap()
            )
            .unwrap_err()
            .code,
            crate::ErrorCode::ConsolidationConflict
        );
        assert_eq!(
            comb.get(&note.parse().unwrap()).unwrap().hash,
            next.note_hash
        );
        assert_eq!(
            comb.get(&"concepts/writer".parse().unwrap()).unwrap().hash,
            next.target_hash
        );
        assert_recorded_recopy(&mut comb, &note, &actor, next);
    }
}

#[test]
fn source_summary_changes_reopen_backlog_without_copying_old_timeline() {
    let (_directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    comb.consolidate(
        &request(&comb, &note),
        &fixture_write(&actor, "first", Some("curation-1")).unwrap(),
    )
    .unwrap();
    let mut source = comb.get(&note.parse().unwrap()).unwrap();
    source.page.compiled = "Revised interpretation".into();
    comb.put_markdown(
        &note.parse().unwrap(),
        &source.page.render().unwrap(),
        Some(&source.hash),
        &fixture_write(&actor, "edit", Some("curation-1")).unwrap(),
    )
    .unwrap();
    assert_eq!(comb.stats().unwrap().inbox, 1);
    comb.consolidate(
        &request(&comb, &note),
        &fixture_write(&actor, "second", Some("curation-1")).unwrap(),
    )
    .unwrap();
    let copied = copy(&comb);
    assert_eq!(copied.compiled.unwrap().summary, "Revised interpretation");
    assert!(copied.entries.is_empty() && copied.original_markdown.is_none());
}

#[test]
fn source_hand_edit_preserves_incremental_content_without_restoring_authorship() {
    let (directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    comb.consolidate(
        &request(&comb, &note),
        &fixture_write(&actor, "first", Some("curation-1")).unwrap(),
    )
    .unwrap();
    hand_edit(directory.path(), &mut comb, &note, |page| {
        page.compiled = "Corrected interpretation".into();
    });
    assert!(
        comb.get(&note.parse().unwrap())
            .unwrap()
            .page
            .metadata
            .owned()
            .consolidated_into()
            .is_empty()
    );
    comb.consolidate(
        &request(&comb, &note),
        &fixture_write(&actor, "corrected", Some("curation-1")).unwrap(),
    )
    .unwrap();
    let copied = copy(&comb);
    assert!(copied.original_markdown.is_none() && copied.entries.is_empty() && !copied.recopy);
    assert_eq!(copied.compiled.unwrap().summary, "Corrected interpretation");
    assert!(
        comb.get(&note.parse().unwrap())
            .unwrap()
            .page
            .timeline
            .iter()
            .all(|entry| entry.metadata.owned().actor() == &Actor::External)
    );
    assert_eq!(comb.stats().unwrap().inbox, 0);
}
