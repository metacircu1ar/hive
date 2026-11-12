use super::*;

#[test]
fn missing_projection_is_corruption_not_complete_empty_coverage() {
    let (_root, comb) = fixture();
    comb.index
        .connection
        .execute(
            "DELETE FROM relation_coverage WHERE source='concepts/c'",
            [],
        )
        .unwrap();
    assert_eq!(
        query(&comb, &request("concepts/a")).unwrap_err().code,
        crate::ErrorCode::IndexError
    );
}

#[test]
fn equal_cardinality_cannot_hide_a_missing_projection_behind_an_orphan() {
    let (_root, comb) = fixture();
    comb.index
        .connection
        .execute_batch(
            "PRAGMA foreign_keys=OFF;
         UPDATE relation_coverage SET source='concepts/absent' WHERE source='concepts/c';
         PRAGMA foreign_keys=ON;",
        )
        .unwrap();
    assert_eq!(
        query(&comb, &request("concepts/a")).unwrap_err().code,
        crate::ErrorCode::IndexError
    );
}

#[test]
fn optional_totals_distinguish_deadline_cancellation_and_storage_failure() {
    let (_root, comb) = fixture();
    let q = request("concepts/a");
    let deadline = Stop::new(0, Cancellation::default());
    let handler = deadline.clone();
    comb.index
        .connection
        .progress_handler(1, Some(move || handler.reason().is_some()))
        .unwrap();
    assert!(
        comb.index
            .graph_optional_totals(&q, crate::search::Filter::default(), &deadline)
            .unwrap()
            .is_none()
    );
    comb.index
        .connection
        .progress_handler(0, None::<fn() -> bool>)
        .unwrap();

    let cancel = Cancellation::default();
    cancel.cancel();
    let cancelled = Stop::new(1000, cancel);
    let handler = cancelled.clone();
    comb.index
        .connection
        .progress_handler(1, Some(move || handler.reason().is_some()))
        .unwrap();
    let error = comb
        .index
        .graph_optional_totals(&q, crate::search::Filter::default(), &cancelled)
        .unwrap_err();
    assert_eq!(cancelled.interruption(&error), Some(Incomplete::Cancelled));
    comb.index
        .connection
        .progress_handler(0, None::<fn() -> bool>)
        .unwrap();

    comb.index
        .connection
        .execute("DROP TABLE relation_coverage", [])
        .unwrap();
    let error = comb
        .index
        .graph_optional_totals(&q, crate::search::Filter::default(), &deadline)
        .unwrap_err();
    assert_eq!(deadline.interruption(&error), None);
}

#[test]
fn a_hundred_thousand_page_projection_keeps_filtered_and_strict_traversal_complete() {
    let (_root, mut comb) = fixture();
    for slug in ["concepts/a", "concepts/b", "concepts/c", "concepts/d"] {
        let page = comb.get(&slug.parse().unwrap()).unwrap();
        put(&mut comb, slug, &page.page.compiled, true);
    }
    // Synthetic index-only ballast: exercise large page payloads without issuing
    // 100,000 durable page publications or invoking any provider/model.
    comb.index
        .connection
        .execute_batch(
            "BEGIN; WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000)
         INSERT INTO pages SELECT printf('concepts/filler-%06d',x),p.title,p.title_key,p.kind,
         p.content_hash,p.verified,p.external,p.updated_at,printf('%02048d',x),p.page_json,0,0
         FROM n CROSS JOIN pages p WHERE p.slug='concepts/d';
         INSERT INTO relation_coverage SELECT slug,1,0,0,0,0,'[]',0 FROM pages
         WHERE slug LIKE 'concepts/filler-%'; COMMIT; PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .unwrap();
    for (reviewed, prefix, strict) in [
        (false, None, false),
        (true, None, false),
        (true, None, true),
        (false, Some("concepts/"), false),
        (false, Some("concepts/"), true),
    ] {
        let mut q = request("concepts/a");
        q.reviewed_only = reviewed;
        q.prefix = prefix.map(str::to_owned);
        q.strict = strict;
        check_large_query(&comb, &q);
        q.to = Some("concepts/d".parse().unwrap());
        check_large_query(&comb, &q);
    }
    indexed_completeness_plan(&comb);
}

fn check_large_query(comb: &Comb, request: &Query) {
    let start = std::time::Instant::now();
    let r = query(comb, request).unwrap();
    eprintln!(
        "100k projection reviewed={}, prefix={:?}, strict={}, connection={}: {:?}, totals={:?}",
        request.reviewed_only,
        request.prefix,
        request.strict,
        request.to.is_some(),
        start.elapsed(),
        r.coverage.totals
    );
    assert!(r.complete, "{:?}", r.incomplete_reasons);
    assert_eq!(r.coverage.current_incomplete_sources, Some(0));
    if request.to.is_some() {
        let path = r.connection.unwrap();
        assert!(path.shortest);
        assert_eq!(path.edges.len(), 2);
    } else {
        assert_eq!(r.relations.len(), 4);
    }
    if let Some(totals) = r.coverage.totals {
        assert_eq!(totals.pages, 100_004);
        assert_eq!(totals.current_occurrences, 4);
    }
}

fn indexed_completeness_plan(comb: &Comb) {
    let sql = format!("EXPLAIN QUERY PLAN {}", sql::INCOMPLETE);
    let mut statement = comb.index.connection.prepare(&sql).unwrap();
    scope::bind(
        &mut statement,
        &request("concepts/a"),
        crate::search::Filter::default(),
        ":reviewed",
        &false,
    )
    .unwrap();
    let plan = statement
        .raw_query()
        .mapped(|r| r.get::<_, String>(3))
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join("\n");
    assert!(
        plan.contains("SEARCH c USING COVERING INDEX relation_coverage_current"),
        "{plan}"
    );
    assert!(!plan.contains("SCAN p"), "{plan}");
    let sql = format!("EXPLAIN QUERY PLAN {}", sql::PROJECTION_MEMBERSHIP);
    let plan = comb
        .index
        .connection
        .prepare(&sql)
        .unwrap()
        .query_map([], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join("\n");
    assert!(
        plan.contains("SCAN p USING COVERING INDEX sqlite_autoindex_pages_1"),
        "{plan}"
    );
    assert!(
        plan.contains("SEARCH c USING COVERING INDEX sqlite_autoindex_relation_coverage_1"),
        "{plan}"
    );
}
