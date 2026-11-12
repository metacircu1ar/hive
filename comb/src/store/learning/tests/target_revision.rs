//! Target revision regressions.
use super::*;

#[test]
fn new_note_can_be_consolidated_into_a_hand_edited_target_without_recopy() {
    let (directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    comb.consolidate(
        &request(&comb, &note),
        &fixture_write(&actor, "first", Some("curation-1")).unwrap(),
    )
    .unwrap();
    hand_edit(directory.path(), &mut comb, "concepts/writer", |page| {
        page.compiled = "Human clarification".into();
    });
    let new = comb
        .capture(
            "A distinct new observation",
            &fixture_write(&actor, "new-note", Some("curation-1")).unwrap(),
        )
        .unwrap()
        .slug;
    let original = comb.get(&new).unwrap().markdown;
    comb.consolidate(
        &request(&comb, new.as_str()),
        &fixture_write(&actor, "new-copy", Some("curation-1")).unwrap(),
    )
    .unwrap();
    let copied = copy(&comb);
    assert!(!copied.recopy);
    assert_eq!(copied.original_markdown.as_deref(), Some(original.as_str()));
    let target = comb.get(&"concepts/writer".parse().unwrap()).unwrap();
    assert_eq!(target.page.timeline.len(), 2);
    assert_eq!(
        target.page.timeline[1].metadata.owned().actor(),
        &Actor::External
    );
}

#[test]
fn lost_coverage_after_interrupted_first_copy_or_both_external_edits_requires_recopy() {
    for interrupted in [false, true] {
        let (directory, mut comb, note) = setup();
        let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
        if interrupted {
            INTERRUPT_AFTER_TARGET.with(|flag| flag.set(true));
        }
        let first = comb.consolidate(
            &request(&comb, &note),
            &fixture_write(&actor, "first", Some("curation-1")).unwrap(),
        );
        if interrupted {
            assert!(first.unwrap_err().file_committed);
            assert_eq!(
                comb.ready().unwrap_err().code,
                crate::ErrorCode::RecoveryRequired
            );
            drop(comb);
            comb = Comb::open(directory.path()).unwrap();
        } else {
            first.unwrap();
        }
        hand_edit(directory.path(), &mut comb, &note, |page| {
            page.compiled = "Edited source".into();
        });
        hand_edit(directory.path(), &mut comb, "concepts/writer", |page| {
            page.compiled = "Edited target".into();
        });
        assert_lost_coverage_requires_recopy(&mut comb, &note, &actor);
    }
}

#[test]
fn target_hand_edit_uses_sealed_source_checkpoint_for_increment_and_interrupted_retry() {
    for appended in [false, true] {
        let (directory, mut comb, note) = setup();
        let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
        comb.consolidate(
            &request(&comb, &note),
            &fixture_write(&actor, "first", Some("curation-1")).unwrap(),
        )
        .unwrap();
        hand_edit(directory.path(), &mut comb, "concepts/writer", |page| {
            page.compiled = "Corrected target summary".into();
        });
        if appended {
            comb.append(
                &note.parse().unwrap(),
                crate::store::AppendEvidence {
                    text: "Second measurement",
                    source: "test:new",
                    compiled: None,
                    expected: None,
                },
                &fixture_write(&actor, "append", Some("curation-1")).unwrap(),
            )
            .unwrap();
        }
        let next = request(&comb, &note);
        INTERRUPT_AFTER_TARGET.with(|flag| flag.set(true));
        assert!(
            comb.consolidate(
                &next,
                &fixture_write(&actor, "second", Some("curation-1")).unwrap()
            )
            .unwrap_err()
            .file_committed
        );
        assert_eq!(
            comb.ready().unwrap_err().code,
            crate::ErrorCode::RecoveryRequired
        );
        drop(comb);
        let mut comb = Comb::rebuild(directory.path()).unwrap();
        let result = comb
            .consolidate(
                &next,
                &fixture_write(&actor, "second", Some("curation-1")).unwrap(),
            )
            .unwrap();
        assert!(result.target.replayed && result.note.replayed);
        assert_retried_checkpoint(&comb, appended);
    }
}
