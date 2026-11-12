//! Embedding concurrency regressions.
use super::*;

#[test]
fn hybrid_propagates_query_and_resident_poison_instead_of_text_fallback() {
    let (_root, mut comb, mut mock) = setup();
    put(&mut comb, "concepts/cat", "Cat", "feline");
    fill(&mut comb, &mut mock);
    let request = request("feline");
    let query = query(&comb, &mut mock, "feline");
    let cache = std::sync::Arc::new(
        crate::index::cache::Resident::new(query.lease.clone(), 1024, comb.health.clone()).unwrap(),
    );
    comb.index = comb.index.with_vectors(Some(cache.clone()));
    assert_eq!(
        comb.index
            .retrieve(
                &request,
                Mode::Hybrid,
                QueryVector::Unavailable(Error::shared_state_poisoned())
            )
            .err()
            .unwrap()
            .code,
        crate::ErrorCode::RecoveryRequired
    );
    assert_eq!(
        comb.index
            .retrieve(
                &request,
                Mode::Hybrid,
                QueryVector::Unavailable(Error::embeddings_busy())
            )
            .unwrap()
            .fallback,
        Some(crate::ErrorCode::EmbeddingsBusy)
    );
    cache.poison_on_next_read();
    assert_eq!(
        comb.index
            .retrieve(&request, Mode::Hybrid, QueryVector::Vector(query))
            .err()
            .unwrap()
            .code,
        crate::ErrorCode::RecoveryRequired
    );
    assert_eq!(
        comb.ready().err().unwrap().code,
        crate::ErrorCode::RecoveryRequired
    );
}

#[test]
fn blocked_query_work_does_not_hold_comb_and_queries_recheck_activation() {
    let (_root, mut comb, mut mock) = setup();
    put(&mut comb, "concepts/cat", "Cat", "feline");
    fill(&mut comb, &mut mock);
    for aba in [false, true] {
        comb.activate_embeddings(mock.space(), true).unwrap();
        let lease = comb.index().unwrap().embedding_lease(mock.space()).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            run_query(
                &mut Blocked {
                    inner: mock_embedder("a"),
                    started: started_tx,
                    resume: resume_rx,
                },
                lease,
                "kitten",
            )
            .unwrap()
        });
        started_rx.recv().unwrap();
        // This mutation and equal-dimensional switch finish before inference is released.
        put(&mut comb, "concepts/other", "Other", "canine");
        comb.activate_embeddings(mock_embedder("b").space(), true)
            .unwrap();
        if aba {
            comb.activate_embeddings(mock.space(), true).unwrap();
        }
        resume_tx.send(()).unwrap();
        let late = handle.join().unwrap();
        let result = comb
            .index()
            .unwrap()
            .retrieve(&request("kitten"), Mode::Hybrid, QueryVector::Vector(late))
            .unwrap();
        assert_eq!(result.mode, Mode::Text);
        assert_eq!(
            result.fallback,
            Some(crate::ErrorCode::EmbeddingSpaceChanged)
        );
    }
}

#[test]
fn cache_poison_during_publication_fences_without_relocking_the_gate() {
    let (done, received) = mpsc::channel();
    let worker = thread::spawn(move || {
        let (_root, mut comb, mut mock) = setup();
        put(&mut comb, "concepts/cat", "Cat", "feline");
        let batch = comb
            .index()
            .unwrap()
            .embedding_jobs(&mut Scan::default(), 32)
            .unwrap();
        let cache = std::sync::Arc::new(
            crate::index::cache::Resident::new(batch.lease.clone(), 1024, comb.health.clone())
                .unwrap(),
        );
        cache.poison_on_next_read();
        comb.index = comb.index.with_vectors(Some(cache));
        let completed = run_batch(&mut mock, batch).unwrap();
        let error = comb.apply_embeddings(completed).unwrap_err();
        assert_eq!(error.code, crate::ErrorCode::RecoveryRequired);
        assert_eq!(
            comb.ready().unwrap_err().code,
            crate::ErrorCode::RecoveryRequired
        );
        // The derived transaction committed before resident publication failed.
        assert_eq!(
            comb.index
                .embedding_coverage(crate::search::Filter::default())
                .unwrap()
                .embedded,
            1
        );
        done.send(()).unwrap();
    });
    received
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    worker.join().unwrap();
}
