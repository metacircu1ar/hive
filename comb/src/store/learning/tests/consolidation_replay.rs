//! Consolidation replay regressions.
use super::*;

#[test]
fn interrupted_marking_replays_after_rebuild() {
    let (directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    let first = interrupt_first_copy(&mut comb, &note, &actor);
    drop(comb);
    let mut comb = Comb::rebuild(directory.path()).unwrap();
    let replay = comb
        .consolidate(
            &first,
            &fixture_write(&actor, "interrupted", Some("curation-1")).unwrap(),
        )
        .unwrap();
    assert!(replay.target.replayed);
    assert!(replay.note.replayed);
    assert_eq!(
        comb.get(&"concepts/writer".parse().unwrap())
            .unwrap()
            .page
            .timeline
            .len(),
        1
    );
    assert_eq!(comb.stats().unwrap().inbox, 0);
}

#[test]
fn successful_consolidation_replays_after_rebuild_and_copies_only_new_evidence() {
    let (directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    let original = comb.get(&note.parse().unwrap()).unwrap().markdown;
    let first = request(&comb, &note);
    comb.consolidate(
        &first,
        &fixture_write(&actor, "fold-1", Some("curation-1")).unwrap(),
    )
    .unwrap();
    assert_first_copy(&comb, &original);
    drop(comb);
    let mut comb = Comb::rebuild(directory.path()).unwrap();
    let replay = comb
        .consolidate(
            &first,
            &fixture_write(&actor, "fold-1", Some("curation-1")).unwrap(),
        )
        .unwrap();
    assert!(replay.target.replayed && replay.note.replayed);
    for i in 0..2 {
        comb.append(
            &note.parse().unwrap(),
            crate::store::AppendEvidence {
                text: &format!("New measurement {i}"),
                source: "test:later",
                compiled: None,
                expected: None,
            },
            &fixture_write(&actor, &format!("append-{i}"), Some("curation-1")).unwrap(),
        )
        .unwrap();
    }
    assert_eq!(comb.stats().unwrap().inbox, 1);
    comb.consolidate(
        &request(&comb, &note),
        &fixture_write(&actor, "fold-2", Some("curation-1")).unwrap(),
    )
    .unwrap();
    let copied = copy(&comb);
    assert!(copied.original_markdown.is_none() && copied.compiled.is_none());
    assert_eq!(copied.entries.len(), 2);
    assert!(
        copied
            .entries
            .iter()
            .all(|e| e.text.starts_with("New measurement"))
    );
    assert_eq!(
        comb.get(&"concepts/writer".parse().unwrap())
            .unwrap()
            .page
            .timeline
            .len(),
        2
    );
    assert_eq!(comb.stats().unwrap().inbox, 0);
}

#[test]
fn source_edit_after_interruption_uses_target_coverage() {
    let (directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    let first = interrupt_first_copy(&mut comb, &note, &actor);
    drop(comb);
    let mut comb = Comb::rebuild(directory.path()).unwrap();
    comb.append(
        &note.parse().unwrap(),
        crate::store::AppendEvidence {
            text: "New evidence after interruption",
            source: "test:after",
            compiled: None,
            expected: None,
        },
        &fixture_write(&actor, "after", Some("curation-1")).unwrap(),
    )
    .unwrap();
    let replay = comb
        .consolidate(
            &first,
            &fixture_write(&actor, "interrupted", Some("curation-1")).unwrap(),
        )
        .unwrap();
    assert!(replay.target.replayed && replay.note.replayed);
    assert_eq!(
        comb.get(&"concepts/writer".parse().unwrap())
            .unwrap()
            .page
            .timeline
            .len(),
        1
    );
    comb.consolidate(
        &request(&comb, &note),
        &fixture_write(&actor, "revised", Some("curation-1")).unwrap(),
    )
    .unwrap();
    let copied = copy(&comb);
    assert!(copied.original_markdown.is_none());
    assert_eq!(copied.entries.len(), 1);
    assert_eq!(copied.entries[0].text, "New evidence after interruption");
    assert_eq!(comb.stats().unwrap().inbox, 0);
}

#[test]
fn source_change_after_target_commit_returns_committed_conflict() {
    let (directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    let request = request(&comb, &note);
    let path = directory.path().join(format!("{}.md", request.note));
    comb.before_index = Some(Box::new(move || {
        std::fs::write(
            path,
            "---\ntitle: Changed source\n---\nNew external evidence.",
        )
        .unwrap();
    }));
    let write = fixture_write(&actor, "target-then-conflict", Some("curation-1")).unwrap();
    let error = comb.consolidate(&request, &write).unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::RecoveryRequired);
    assert!(error.file_committed);
    assert!(
        comb.disk
            .receipt(&write.principal, &write.request_id)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        comb.ready().unwrap_err().code,
        crate::ErrorCode::RecoveryRequired
    );
    let edited = std::fs::read(directory.path().join(format!("{}.md", request.note))).unwrap();
    drop(comb);
    let error = Comb::open(directory.path()).err().unwrap();
    assert!(error.file_committed);
    assert_eq!(error.code, crate::ErrorCode::RecoveryRequired);
    assert_eq!(
        std::fs::read(directory.path().join(format!("{}.md", request.note))).unwrap(),
        edited
    );
    assert!(directory.path().join(".comb/intent.json").exists());
}
