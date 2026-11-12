//! Search results regressions.
use super::*;

#[test]
fn vector_filters_precede_ranking_and_pages_collapse_before_fusion() {
    let (_root, mut comb, mut mock) = setup();
    put(
        &mut comb,
        "concepts/alpha",
        "Alpha",
        "feline\n\n## More\nfeline again\n\n## Third\nfeline third",
    );
    put(&mut comb, "concepts/beta", "Beta", "feline");
    put(&mut comb, "reports/gamma", "Gamma", "feline");
    fill(&mut comb, &mut mock);
    let mut req = request("kitten");
    req.prefix = Some("concepts/".into());
    req.limit = 2;
    let q = query(&comb, &mut mock, &req.query);
    let result = comb
        .index()
        .unwrap()
        .retrieve(&req, Mode::Vector, QueryVector::Vector(q))
        .unwrap();
    assert_eq!(
        result
            .results
            .iter()
            .map(|h| h.slug.as_str())
            .collect::<Vec<_>>(),
        ["concepts/alpha", "concepts/beta"]
    );
    assert!((result.results[0].score - 1.0 / 61.0).abs() < 1e-12);
    assert!((result.results[1].score - 1.0 / 62.0).abs() < 1e-12);
    assert_eq!(result.coverage.unwrap().total, 4);
    req.query = "feline".into();
    let q = query(&comb, &mut mock, &req.query);
    let result = comb
        .index()
        .unwrap()
        .retrieve(&req, Mode::Hybrid, QueryVector::Vector(q))
        .unwrap();
    // Independently known equal-vector ranks and one text rank per page.
    assert_eq!(result.results.len(), 2);
    assert!(
        result
            .results
            .iter()
            .all(|h| h.matched_by == Route::TextVector)
    );
    assert!(
        (result.results.iter().map(|h| h.score).sum::<f64>() - 2.0 / 61.0 - 2.0 / 62.0).abs()
            < 1e-12
    );
}

#[test]
fn only_final_hits_load_chunk_text_in_all_search_modes() {
    let (_root, mut comb, mut mock) = setup();
    for n in 0..12 {
        put(
            &mut comb,
            &format!("concepts/page-{n}"),
            "Large body",
            &format!("feline {}", "x".repeat(60_000)),
        );
    }
    fill(&mut comb, &mut mock);
    for mode in [Mode::Text, Mode::Vector, Mode::Hybrid] {
        crate::index::search::HYDRATED.set(0);
        let mut req = request("feline");
        req.limit = 1;
        let q = query(&comb, &mut mock, &req.query);
        let results = comb
            .index()
            .unwrap()
            .retrieve(&req, mode, QueryVector::Vector(q))
            .unwrap();
        assert_eq!(results.results.len(), 1);
        assert_eq!(crate::index::search::HYDRATED.get(), 1);
        assert!(results.results[0].snippet.chars().count() <= 480);
    }
}

#[test]
fn doctor_probes_embedding_failure_overflow_beyond_the_finding_limit() {
    let (_root, mut comb, mock) = setup();
    put(&mut comb, "concepts/failures", "Failures", "# Failures");
    let lease = comb.index().unwrap().embedding_lease(mock.space()).unwrap();
    // Populate only derived state to avoid 1025 unrelated durable writes.
    comb.index
        .seed_embedding_failures(&lease.fingerprint)
        .unwrap();
    assert_eq!(
        comb.index().unwrap().embedding_findings().unwrap().len(),
        1025
    );
    let report = comb.doctor().unwrap();
    assert_eq!(report.findings.len(), 1024);
    assert!(report.findings_truncated);
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.kind == FindingKind::EmbeddingRejected)
    );
    assert_eq!(report.embeddings.rejected, 1025);
}

#[test]
fn partial_coverage_preserves_semantic_matches_and_reports_rejections_and_pending() {
    let (_root, mut comb, mut mock) = setup();
    put(&mut comb, "concepts/cat", "Cat", "feline");
    put(&mut comb, "concepts/huge", "Huge", "OVERSIZED");
    fill(&mut comb, &mut mock);
    let q = query(&comb, &mut mock, "kitten");
    let result = comb
        .index()
        .unwrap()
        .retrieve(&request("kitten"), Mode::Hybrid, QueryVector::Vector(q))
        .unwrap();
    assert_eq!(result.mode, Mode::Hybrid);
    assert_eq!(result.results[0].slug, "concepts/cat".parse().unwrap());
    assert_eq!(result.results[0].create_safety, CreateSafety::Unknown);
    assert_eq!(
        result.coverage.unwrap(),
        Coverage {
            total: 2,
            embedded: 1,
            rejected: 1,
            pending: 0
        }
    );
    assert!(
        comb.doctor()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::EmbeddingRejected)
    );
    put(&mut comb, "concepts/new", "New", "feline pending");
    let q = query(&comb, &mut mock, "kitten");
    let result = comb
        .index()
        .unwrap()
        .retrieve(&request("kitten"), Mode::Hybrid, QueryVector::Vector(q))
        .unwrap();
    assert_eq!(result.mode, Mode::Hybrid);
    assert_eq!(result.coverage.unwrap().pending, 1);
    fill(&mut comb, &mut mock);
    assert_eq!(coverage(&comb).embedded, 2);
    comb.retry_embedding_failures().unwrap();
    assert_eq!(
        comb.index()
            .unwrap()
            .embedding_jobs(&mut Scan::default(), 32)
            .unwrap()
            .jobs
            .len(),
        1
    );
}
