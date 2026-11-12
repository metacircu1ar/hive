//! Vector cache regressions.
use super::*;

#[test]
fn rebuild_reuses_vectors_and_only_changed_inputs_need_inference() {
    let (root, mut comb, mut mock) = setup();
    put(&mut comb, "concepts/cat", "Cat", "feline");
    put(&mut comb, "concepts/dog", "Dog", "canine");
    fill(&mut comb, &mut mock);
    let q = query(&comb, &mut mock, "kitten");
    let before = comb
        .index()
        .unwrap()
        .retrieve(&request("kitten"), Mode::Vector, QueryVector::Vector(q))
        .unwrap();
    drop(comb);
    let mut comb = Comb::rebuild(root.path()).unwrap();
    comb.activate_embeddings(mock.space(), false).unwrap();
    let calls = mock.input_count();
    fill(&mut comb, &mut mock);
    assert_eq!(mock.input_count(), calls);
    let q = query(&comb, &mut mock, "kitten");
    let after = comb
        .index()
        .unwrap()
        .retrieve(&request("kitten"), Mode::Vector, QueryVector::Vector(q))
        .unwrap();
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(after).unwrap()
    );
    put(&mut comb, "concepts/dog", "Dog", "different canine input");
    let calls = mock.input_count();
    fill(&mut comb, &mut mock);
    assert_eq!(mock.input_count(), calls + 1);
}

#[test]
fn invalid_vectors_and_corrupt_cache_never_reach_search() {
    for vector in [
        vec![0.0, 0.0],
        vec![f32::NAN, 1.0],
        vec![f32::INFINITY, 0.0],
        vec![1.0],
    ] {
        assert!(normalize(vector, 2).is_err());
    }
    let (_root, mut comb, mut mock) = setup();
    put(&mut comb, "concepts/cat", "Cat", "feline");
    let batch = comb
        .index()
        .unwrap()
        .embedding_jobs(&mut Scan::default(), 32)
        .unwrap();
    assert!(
        comb.apply_embeddings(Completed {
            batch,
            vectors: vec![Ok(vec![0.0, 0.0])]
        })
        .is_err()
    );
    fill(&mut comb, &mut mock);
    comb.index.corrupt_cached_vectors().unwrap();
    let q = query(&comb, &mut mock, "kitten");
    assert_eq!(
        comb.index()
            .unwrap()
            .retrieve(&request("kitten"), Mode::Vector, QueryVector::Vector(q))
            .err()
            .unwrap()
            .code,
        crate::ErrorCode::InvalidEmbedding
    );
    let q = query(&comb, &mut mock, "kitten");
    assert_eq!(
        comb.index()
            .unwrap()
            .retrieve(&request("kitten"), Mode::Hybrid, QueryVector::Vector(q))
            .unwrap()
            .fallback,
        Some(crate::ErrorCode::InvalidEmbedding)
    );
}
