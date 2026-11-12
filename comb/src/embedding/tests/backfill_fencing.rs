//! Backfill fencing regressions.
use super::*;

#[test]
fn backfill_selection_work_is_linear() {
    let small = measure(512);
    let large = measure(1024);
    assert!(
        large < small * 23 / 10,
        "Selection VM steps should scale linearly: {small} -> {large}"
    );
}

#[test]
fn delayed_jobs_are_fenced_after_edit_delete_recreate_and_model_aba() {
    let (_root, mut comb, mut mock) = setup();
    put(&mut comb, "concepts/cat", "Cat", "feline");
    let old = complete_pending(&comb, &mut mock);
    put(&mut comb, "concepts/cat", "Cat", "canine");
    assert_eq!(comb.apply_embeddings(old).unwrap().stale, 1);
    let old = complete_pending(&comb, &mut mock);
    let hash = comb.get(&"concepts/cat".parse().unwrap()).unwrap().hash;
    comb.remove(
        &"concepts/cat".parse().unwrap(),
        &hash,
        &Write::new(&Actor::Human, &"remove-cat".parse().unwrap(), None).unwrap(),
    )
    .unwrap();
    put(&mut comb, "concepts/cat", "Cat", "canine");
    assert_eq!(comb.apply_embeddings(old).unwrap().stale, 1);
    let old = complete_pending(&comb, &mut mock);
    let other = mock_embedder("b");
    assert_eq!(
        comb.activate_embeddings(other.space(), false)
            .unwrap_err()
            .code,
        crate::ErrorCode::EmbeddingReplaceRequired
    );
    comb.activate_embeddings(other.space(), true).unwrap();
    comb.activate_embeddings(mock.space(), true).unwrap();
    assert_eq!(comb.apply_embeddings(old).unwrap().stale, 1);
    assert_eq!(coverage(&comb).embedded, 0);
}

#[test]
fn cursor_wrap_preserves_edits_and_model_switch() {
    let (_root, mut comb, mut mock) = setup();
    put(
        &mut comb,
        "concepts/corpus",
        "Corpus",
        &many_chunks(40, "old"),
    );
    let mut scan = Scan::default();
    let held = run_batch(
        &mut mock,
        comb.index().unwrap().embedding_jobs(&mut scan, 16).unwrap(),
    )
    .unwrap();
    put(
        &mut comb,
        "concepts/corpus",
        "Corpus",
        &many_chunks(40, "new"),
    );
    assert_eq!(comb.apply_embeddings(held).unwrap().stale, 16);
    // The changed first 16 rows may be behind the cursor; finish the pass, then
    // wrap and find them. A service retries a pass after mutation notification.
    for _ in 0..2 {
        loop {
            let batch = comb.index().unwrap().embedding_jobs(&mut scan, 16).unwrap();
            if batch.jobs.is_empty() {
                break;
            }
            comb.apply_embeddings(run_batch(&mut mock, batch).unwrap())
                .unwrap();
        }
    }
    assert_eq!(coverage(&comb).embedded, 40);
    let other = mock_embedder("other");
    comb.activate_embeddings(other.space(), true).unwrap();
    assert_eq!(
        comb.index()
            .unwrap()
            .embedding_jobs(&mut scan, 16)
            .unwrap()
            .jobs
            .len(),
        16
    );
}
