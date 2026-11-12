//! Hosted graph schemas and reads survive native conversation lifecycles.
use super::*;

provider_matrix!(native_graph_catalog_binding_and_lifecycle, graph_case);

fn graph_actions() -> Value {
    json!([
        {"catalog":true},
        {"name":"comb_graph_query","arguments":{
            "anchor":"concepts/graph-a","to":"concepts/graph-c","predicates":["depends_on"]}},
        {"name":"comb_graph_query","arguments":{
            "anchor":"concepts/graph-a","to":"concepts/graph-c","predicates":["depends_on"],
                "reviewed_only":true}},
        {"name":"comb_graph_query","arguments":{
            "anchor":"concepts/graph-a","predicates":["depends_on"],"claim":{"id":"forged"}}},
        {"name":"comb_search","arguments":{"query":"unmatched-needle","mode":"text","graph":{
            "anchor":"concepts/graph-a","predicates":["depends_on"]}}},
        {"name":"comb_backlinks","arguments":{"slug":"concepts/graph-c"}}
    ])
}

fn graph_case(provider: Provider) {
    let comb = CombService::new();
    let mut f = Harness::builder(provider).build();
    f.edit_config(|config| {
        config["comb"] = json!({"credential_file":comb.credential});
        config["runtime"] = json!({"max_loaded":8,"max_providers":2,
            "sessions_per_codex_process":4,"max_starting":2,"max_active_turns":8});
        for role in ["reviewer", "claude-reviewer"] {
            if !config["roles"][role].is_null() {
                config["roles"][role]["comb"] = json!({"credential_file":comb.credential,
                    "reviewer":true});
            }
        }
    });
    f.start();
    let writer = spawn(
        &f,
        provider,
        false,
        json!([
            {"name":"comb_put","arguments":{"slug":"concepts/graph-a","markdown":"---\ntitle: \
                Graph A\n---\n## Relations\n- [[depends_on::concepts/graph-b]]"}},
            {"name":"comb_put","arguments":{"slug":"concepts/graph-b","markdown":"---\ntitle: \
                Graph B\n---\n## Relations\n- [[depends_on::concepts/graph-c]]"}},
            {"name":"comb_put","arguments":{"slug":"concepts/graph-c",
                "markdown":"---\ntitle: Graph C\n---\nTarget"}}
        ]),
    );
    assert!(results(&f, &writer, 1).iter().all(|r| r["error"].is_null()));
    let source = comb.get("concepts/graph-a");
    assert_eq!(source["page"]["metadata"]["author"]["id"], writer);
    let reader = spawn(&f, provider, false, graph_actions());
    check_graph_replies(results(&f, &reader, 1), false);
    let reviewer = spawn(&f, provider, true, graph_actions());
    check_graph_replies(results(&f, &reviewer, 1), true);
    assert_eq!(comb.get("concepts/graph-a")["hash"], source["hash"]);
    let thread = f.settled(&reader).thread_id.clone();
    f.shutdown();
    f.edit_config(|config| {
        config["comb"]["credential_file"] = json!("/no/such/replacement-graph-binding.json");
    });
    f.start();
    f.ok(json!({"op":"send","target":reader,"body":body(graph_actions())}));
    f.resume_after_wakeup(&reader);
    check_graph_replies(results(&f, &reader, 2), false);
    assert_eq!(f.settled(&reader).thread_id, thread);
    let branch = f.spawned(json!({"op":"spawn","from_agent":reader,"body":body(graph_actions())}));
    let child = branch.agent_id.as_str();
    check_graph_replies(results(&f, child, 1), false);
    assert_ne!(f.settled(child).thread_id, thread);
    let fresh = spawn(&f, provider, false, graph_actions());
    assert_eq!(results(&f, &fresh, 1)[1]["error"]["code"], "io_error");
    rejected_credentials(&comb, &f, &reader, thread.as_deref());
    f.shutdown();
}

fn check_graph_replies(replies: Vec<Value>, reviewer: bool) {
    check_catalog(&replies[0], reviewer);
    let report: ::comb::graph::Report = serde_json::from_value(replies[1].clone()).unwrap();
    assert!(report.complete);
    let path = report.connection.unwrap();
    assert!(path.shortest && path.unverified);
    assert_eq!(
        path.nodes
            .iter()
            .map(::comb::values::Slug::as_str)
            .collect::<Vec<_>>(),
        ["concepts/graph-a", "concepts/graph-b", "concepts/graph-c"]
    );
    assert_eq!(path.edges.len(), 2);
    assert!(
        path.edges
            .iter()
            .all(|e| e.declaration.unverified && e.page_unverified)
    );
    assert_eq!(
        path.edges[0].declaration.text,
        "[[depends_on::concepts/graph-b]]"
    );
    let filtered: ::comb::graph::Report = serde_json::from_value(replies[2].clone()).unwrap();
    assert!(filtered.complete && filtered.connection.is_none());
    assert!(
        replies[3]["error"].is_object(),
        "forged authority must be rejected"
    );
    let search: ::comb::search::Results = serde_json::from_value(replies[4].clone()).unwrap();
    assert_eq!(search.results.len(), 2);
    assert!(
        search
            .results
            .iter()
            .all(|hit| hit.matched_by == ::comb::values::Route::Graph)
    );
    assert_eq!(search.graph.unwrap().relations.len(), 2);
    let links: ::comb::links::Report = serde_json::from_value(replies[5].clone()).unwrap();
    assert!(links.complete);
    assert_eq!(links.references[0].slug.as_str(), "concepts/graph-b");
    assert!(links.references[0].typed[0].unverified);
}

fn check_catalog(reply: &Value, reviewer: bool) {
    let expected = if reviewer {
        include_str!("../../../comb/tests/fixtures/contracts/hosted-catalog-true.json")
    } else {
        include_str!("../../../comb/tests/fixtures/contracts/hosted-catalog-false.json")
    };
    let mut expected: Vec<Value> = serde_json::from_str(expected).unwrap();
    let mut actual: Vec<Value> = reply["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["name"].as_str().unwrap().starts_with("comb_"))
        .cloned()
        .collect();
    for tool in expected.iter_mut().chain(actual.iter_mut()) {
        tool.as_object_mut().unwrap().remove("type");
    }
    assert_eq!(
        actual, expected,
        "native discovery must retain complete schemas"
    );
}

fn rejected_credentials(comb: &CombService, f: &Harness, reader: &str, thread: Option<&str>) {
    let connection = Connection::read(&comb.credential).unwrap();
    let mut wrong = connection.clone();
    wrong.credential.comb_id = ::comb::values::CombId::generate();
    support::publish_fixture(&comb.credential, &serde_json::to_vec(&wrong).unwrap());
    f.ok(json!({"op":"send","target":reader,"body":body(graph_actions())}));
    assert_eq!(results(f, reader, 3)[1]["error"]["code"], "forbidden");
    support::publish_fixture(&comb.credential, &serde_json::to_vec(&connection).unwrap());
    comb.request(Operation::CredentialRevoke {
        id: connection.credential.id.clone(),
    });
    f.ok(json!({"op":"send","target":reader,"body":body(graph_actions())}));
    assert_eq!(results(f, reader, 4)[1]["error"]["code"], "forbidden");
    assert_eq!(f.settled(reader).thread_id.as_deref(), thread);
}
