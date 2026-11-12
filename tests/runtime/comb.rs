//! Real Comb service behind deterministic native Codex/Claude conversations.
use super::*;
use ::comb::service::auth::ConnectionExt as _;
use ::comb::service::{Operation, Options, Server, auth::Connection};
use std::os::unix::fs::PermissionsExt;
mod collectors;
mod diagnostics;
mod graph;
mod outputs;

mod fixture;
use fixture::{CombService, body, results, spawn};

provider_matrix!(
    ordinary_curator_consolidates_and_reviewer_decides,
    curator_case
);
fn curator_case(provider: Provider) {
    let comb = CombService::new();
    let note = comb.request(Operation::Capture {
        text: "Observed writer contention in the release run.".into(),
        request_id: "observation".parse().unwrap(),
        run: None,
    });
    let target = comb.request(Operation::Put(comb_protocol::service::api::PutRequest {
        slug: "concepts/writer".parse().unwrap(),
        markdown: "---\ntitle: Writer\n---\nInitial summary".into(),
        expected_hash: None,
        request_id: "target".parse().unwrap(),
        run: None,
    }));
    let mut f = Harness::builder(provider).build();
    configure_curator(&mut f, &comb, provider);
    f.start();
    let actions = json!([
        {"name":"comb_search","arguments":{"query":"writer","mode":"text"}},
        {"name":"comb_consolidate","arguments":{"note":note["slug"],"target":"concepts/writer",
            "note_hash":note["hash"],"target_hash":target["hash"],
                "summary":"Release measurements show contention. [evidence:new]",
            "run":"ordinary-curator","request_id":"fold"}},
        {"name":"comb_put","arguments":{"slug":"review/writer-duplicate","markdown":"---\ntitle: \
            Possible duplicate\n---\nCompare writer component pages before \
            merging.",
                "run":"ordinary-curator"}}
    ]);
    let curator = f.ok(json!({"op":"spawn","role":"curator","cwd":f.tmp.path(),
        "body":body(actions)}))["agent_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let replies = results(&f, &curator, 1);
    assert!(replies.iter().all(|r| r["error"].is_null()), "{replies:?}");
    let target = comb.get("concepts/writer");
    assert_eq!(
        target["page"]["timeline"][0]["metadata"]["actor"]["id"],
        curator
    );
    assert_eq!(
        target["page"]["timeline"][0]["metadata"]["run"],
        "ordinary-curator"
    );
    let checkpoint = comb.get(note["slug"].as_str().unwrap());
    assert_eq!(
        checkpoint["page"]["metadata"]["consolidated_into"][0]["by"]["id"],
        curator
    );
    let proposal = comb.get("review/writer-duplicate");
    let decide = json!([{"name":"comb_decide","arguments":{"slug":"review/writer-duplicate",
        "expected_hash":proposal["hash"],"decision":"reject",
            "reason":"Pages describe different responsibilities.","run":"review-curator"}}]);
    f.ok(json!({"op":"send","target":curator,"body":body(decide.clone())}));
    assert_eq!(results(&f, &curator, 2)[0]["error"]["code"], "forbidden");
    let reviewer = spawn(&f, provider, true, decide);
    assert!(results(&f, &reviewer, 1)[0]["error"].is_null());
    assert_eq!(
        comb.request(Operation::Doctor)["stats"]["unresolved_proposals"],
        0
    );
    f.ok(json!({"op":"send","target":curator,"body":body(json!([
        {"name":"comb_put","arguments":{"slug":"reports/curator","markdown":"---\ntitle: \
            Curation report\n---\nFolded the writer note and requested explicit duplicate \
            review.",
                "run":"ordinary-curator"}},
        {"name":"hive_complete",
            "arguments":{"body":"Consolidated the note; reviewer decided the proposal.",
                "request_id":"complete-curation"}}
    ]))}));
    let replies = results(&f, &curator, 3);
    assert!(replies.iter().all(|r| r["error"].is_null()), "{replies:?}");
    f.wait(&curator, |a| a.task_outcome == "completed");
    assert_eq!(comb.request(Operation::Stats)["inbox"], 0);
    f.shutdown();
}
provider_matrix!(
    native_comb_attribution_replay_promotion_and_saved_binding,
    native_comb_case
);
fn native_comb_case(provider: Provider) {
    let comb = CombService::new();
    let mut f = Harness::builder(provider).build();
    configure_native_comb(&mut f, &comb);
    f.start();
    let append = json!({"name":"comb_append","arguments":{"slug":"concepts/shared",
        "text":"Measured observation","source":"test:fixture"},"native_id":"same-callback"});
    let writer = spawn(
        &f,
        provider,
        false,
        json!([
            {"name":"comb_put","arguments":{"slug":"concepts/shared",
                "markdown":"---\ntitle: Shared\n---\nInitial summary"},"native_id":"create"},
            append, append,
            {"name":"comb_append","arguments":{"slug":"concepts/shared",
                "text":"Measured observation","source":"test:fixture"},
                    "native_id":"different-callback"}
        ]),
    );
    let replies = results(&f, &writer, 1);
    assert!(replies.iter().all(|v| v["error"].is_null()), "{replies:?}");
    assert_eq!(
        replies[2]["replayed"], true,
        "Original native callback must retain its mutation ID"
    );
    assert_ne!(replies[1]["request_id"], replies[3]["request_id"]);
    let page = comb.get("concepts/shared");
    assert_eq!(page["page"]["metadata"]["author"]["id"], writer);
    assert_eq!(page["page"]["timeline"].as_array().unwrap().len(), 2);
    let peer = spawn(
        &f,
        provider,
        false,
        json!([{ "name":"comb_append", "arguments":{"slug":"concepts/shared",
            "text":"Peer observation","source":"test:peer"} }]),
    );
    results(&f, &peer, 1);
    if provider == Provider::Codex {
        assert_eq!(
            f.status(&writer).provider_id,
            f.status(&peer).provider_id,
            "Attribution must survive a shared native process"
        );
        let rejected = f.call(json!({"op":"spawn","from_agent":writer,"role":"reviewer",
            "lifetime":"independent","body":"new tool profile"}));
        let rejected = support::reply::Failure::parse(rejected);
        assert_eq!(rejected.error.code, "binding_conflict");
    }
    let page = comb.get("concepts/shared");
    assert!(
        page["page"]["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["metadata"]["actor"]["id"] == peer),
        "{page}"
    );
    let promote = json!([{"name":"comb_promote","arguments":{"slug":"concepts/shared",
        "expected_hash":page["hash"]}}]);
    f.ok(json!({"op":"send","target":writer,"body":body(promote.clone())}));
    assert_eq!(
        results(&f, &writer, 2)[0]["error"]["code"],
        "forbidden",
        "Saved role ceiling must constrain a broader bridge grant"
    );
    let reviewer = spawn(&f, provider, true, promote);
    assert!(results(&f, &reviewer, 1)[0]["error"].is_null());
    assert_eq!(comb.get("concepts/shared")["unverified"], false);
    saved_binding_after_restart(&mut f, provider, &writer);
}

fn saved_binding_after_restart(f: &mut Harness, provider: Provider, writer: &str) {
    f.shutdown();
    f.edit_config(|config| {
        config["comb"]["credential_file"] = json!("/no/such/new-comb.json");
    });
    f.start();
    f.ok(
        json!({"op":"send","target":writer,"body":body(json!([{"name":"comb_get",
        "arguments":{"slug":"concepts/shared"}}]))}),
    );
    f.resume_after_wakeup(writer);
    assert_eq!(
        results(f, writer, 3)[0]["page"]["metadata"]["title"],
        "Shared"
    );
    let branched = f.spawned(json!({"op":"spawn","from_agent":writer,
        "body":body(json!([{"name":"comb_get","arguments":{"slug":"concepts/shared"}}]))}));
    let child = branched.agent_id.as_str();
    assert_eq!(
        results(f, child, 1)[0]["page"]["metadata"]["title"],
        "Shared"
    );
    let unavailable = spawn(
        f,
        provider,
        false,
        json!([{"name":"comb_get","arguments":{"slug":"concepts/shared"}}]),
    );
    assert_eq!(results(f, &unavailable, 1)[0]["error"]["code"], "io_error");
    f.shutdown();
}

provider_matrix!(
    configured_stdio_mcp_reaches_native_providers,
    configured_stdio_case
);
fn configured_stdio_case(provider: Provider) {
    let mut f = Harness::builder(provider).build();
    let script = f.tmp.path().join("stdio_fixture.py");
    fs::write(
        &script,
        "import sys,json,os\nfor line in sys.stdin:\n    f=json.loads(line)\n    if 'id' not \
            in f: continue\n    if f['method']=='initialize': \
            r={'protocolVersion':'2025-11-25','capabilities':{'tools':{}},'serverInfo':{'name'\
            :'fixture','version':'1'}}\n    elif f['method']=='tools/list': \
            r={'tools':[{'name':'echo','description':'fixture','inputSchema':{'type':'object'}\
            }]}\n    elif f['method']=='tools/call':\n        assert \
            os.environ['MCP_FIXTURE_SECRET']=='configured-secret'\n        \
            r={'content':[{'type':'text','text':f['params']['arguments']['text']}]}\n    \
            else: raise AssertionError(f)\n    print(json.dumps({'jsonrpc':'2.0','id':f['id'],\
            'result':r}),flush=True)\n",
    )
    .unwrap();
    // Use the same resolved Python selected for native provider mocks.
    let command = f.tmp.path().join("python3");
    f.edit_config(|config| {
        config["mcp_servers"] = json!({"docs":{"command":command,"args":[script],
            "env":{"MCP_FIXTURE_SECRET":"configured-secret"}}});
    });
    f.start();
    let id = if provider == Provider::Claude {
        super::claude::spawn(&f, "mcp:docs")
    } else {
        f.spawn("mcp:docs")
    };
    super::claude::answered(&f, &id, "configured stdio works");
    let status = f.status(&id);
    assert_eq!(
        serde_json::to_value(&status.mcp_servers).unwrap(),
        json!(["docs"])
    );
    assert!(
        !serde_json::to_value(&status)
            .unwrap()
            .to_string()
            .contains("configured-secret")
    );
    f.settled(&id);
    f.shutdown();
    let reports = f.reports();
    assert!(!reports.is_empty());
    assert!(
        reports.iter().all(|r| r["confirmed"] == true),
        "{reports:?}"
    );
    f.edit_config(|config| {
        config["mcp_servers"] = json!({});
    });
    f.start();
    f.resume_when_released(&id);
    f.ok(json!({"op":"send","target":id,"body":"mcp:docs"}));
    support::wait::wait_until(
        Duration::from_secs(10),
        Duration::from_millis(25),
        "saved MCP after restart",
        || {
            (f.events(&id)
                .events
                .iter()
                .filter(|e| e.data["text"] == "configured stdio works")
                .count()
                >= 2)
                .then_some(())
        },
    );
    f.shutdown();
}

provider_matrix!(
    held_comb_does_not_block_interrupt_or_other_agent,
    held_comb_case
);
fn held_comb_case(provider: Provider) {
    use std::io::BufRead;
    let private = tempfile::tempdir_in("/tmp").unwrap();
    let socket = private.path().join("held.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let credential = private.path().join("held.json");
    Connection {
        format: hive_input::compatibility::Exact,
        socket,
        credential: ::comb::service::auth::Credential {
            comb_id: ::comb::values::CombId::generate(),
            id: ::comb::values::CredentialId::generate(),
            secret: "fixture".into(),
        },
    }
    .write(&credential)
    .unwrap();
    let (seen, entered) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let mut accepted = None;
        while accepted.is_none() {
            if !matches!(
                released.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ) {
                return;
            }
            if let Ok((socket, _)) = listener.accept() {
                accepted = Some(socket);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut socket = std::io::BufReader::new(accepted.unwrap());
        socket
            .get_mut()
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut frame = String::new();
        socket.read_line(&mut frame).unwrap();
        seen.send(serde_json::from_str::<Value>(&frame).unwrap())
            .unwrap();
        let _ = released.recv_timeout(Duration::from_secs(15));
    });
    let mut f = Harness::builder(provider).build();
    f.edit_config(|config| config["comb"] = json!({"credential_file":credential}));
    f.start();
    let id = spawn(
        &f,
        provider,
        false,
        json!([{"name":"comb_get","arguments":{"slug":"concepts/held"}}]),
    );
    let request = entered.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(request["claim"]["id"], id);
    let start = Instant::now();
    let other = if provider == Provider::Claude {
        super::claude::spawn(&f, "reply:progress")
    } else {
        f.spawn("reply:progress")
    };
    super::claude::answered(&f, &other, "reply:progress");
    f.ok(json!({"op":"interrupt","target":id}));
    f.wait_timeout(&id, Duration::from_secs(5), |a| {
        a.state == "idle" && a.turn_id.is_none()
    });
    assert!(start.elapsed() < Duration::from_secs(5));
    f.ok(json!({"op":"send","target":id,"body":"reply:after-interrupt"}));
    super::claude::answered(&f, &id, "reply:after-interrupt");
    release.send(()).unwrap();
    server.join().unwrap();
    f.shutdown();
}

provider_matrix!(
    collector_captures_mock_evidence_and_completion,
    collector_case
);
fn collector_case(provider: Provider) {
    let comb = CombService::new();
    let mut f = Harness::builder(provider).build();
    f.edit_config(|config| {
        config["comb_collectors"] = json!({"shared":{"credential_file":comb.credential,
            "sources":["review","task_completed"]}});
    });
    f.start();
    let review = json!({"name":"hive_evidence","arguments":{"kind":"review",
        "summary":"Found a missing negative-input check.","files":["factorial.py"],
            "verdict":"findings","request_id":"review-observation"}});
    let agent = spawn(
        &f,
        provider,
        false,
        json!([
            review.clone(), review,
            {"name":"hive_evidence","arguments":{"kind":"correction",
                "summary":"Cannot impersonate a human correction.","request_id":"not-human"}},
            {"name":"hive_complete",
                "arguments":{
                    "body":"PRIVATE completion result must stay out of shared evidence.",
                        "request_id":"complete"}}
        ]),
    );
    let replies = results(&f, &agent, 1);
    assert!(replies[0]["error"].is_null(), "{replies:?}");
    assert_eq!(replies[0], replies[1], "same admission replays");
    assert_eq!(replies[2]["error"]["code"], "invalid_value");
    assert_eq!(replies[2]["error"]["field"], "/kind");
    f.wait(&agent, |a| a.task_outcome == "completed");
    support::wait::wait_until(
        Duration::from_secs(10),
        Duration::from_millis(50),
        "collector drain",
        || {
            (comb.request(Operation::Stats)["inbox"] == 2
                && f.ok(json!({"op":"collectors"}))["collectors"][0]["pending"] == 0)
                .then_some(())
        },
    );
    let pages = comb.request(Operation::List {
        request: ::comb::store::List {
            prefix: Some("inbox/".into()),
            ..Default::default()
        },
    });
    let encoded = pages.to_string();
    assert!(!encoded.contains("PRIVATE"));
    for page in pages.as_array().unwrap() {
        let page = comb.get(page["slug"].as_str().unwrap());
        assert!(!page.to_string().contains("PRIVATE"));
        assert_eq!(page["page"]["metadata"]["author"]["kind"], "collector");
        assert!(page["markdown"].as_str().unwrap().contains(&agent));
    }
    // A human correction gets an explicit not-selected disposition, with stable replay.
    let correction =
        json!({"op":"evidence","kind":"correction","summary":"Human-selected public correction."});
    let first = f.call_id("correction", correction.clone());
    assert_eq!(first["destinations"]["shared"], "not_selected");
    assert_eq!(f.call_id("correction", correction), first);
    f.shutdown();
}

provider_matrix!(
    collector_pending_evidence_survives_purge_and_restart,
    collector_retention_case
);
fn collector_retention_case(provider: Provider) {
    let comb = CombService::new();
    let mut f = Harness::builder(provider).build();
    f.edit_config(|config| {
        config["comb_collectors"] = json!({"shared":{
        "credential_file":comb.credential,"sources":["review","task_completed"]}});
    });
    f.start();
    let hidden = comb.credential.with_extension("hidden");
    fs::rename(&comb.credential, &hidden).unwrap();
    let agent = spawn(
        &f,
        provider,
        false,
        json!([
            {"name":"hive_evidence","arguments":{"kind":"review",
                "summary":"Pending public finding.","verdict":"findings","request_id":"finding"}},
            {"name":"hive_complete","arguments":{"body":"Private completion","request_id":"done"}}
        ]),
    );
    assert!(results(&f, &agent, 1).iter().all(|r| r["error"].is_null()));
    f.wait(&agent, |a| {
        a.task_outcome == "completed" && a.state == "stopped"
    });
    f.ok(json!({"op":"purge","target":agent}));
    assert_eq!(
        f.ok(json!({"op":"collectors"}))["collectors"][0]["pending"],
        2
    );
    f.shutdown();
    fs::rename(hidden, &comb.credential).unwrap();
    f.start();
    support::wait::wait_until(
        Duration::from_secs(10),
        Duration::from_millis(50),
        "retained evidence delivered after restart",
        || {
            (comb.request(Operation::Stats)["inbox"] == 2
                && f.ok(json!({"op":"collectors"}))["collectors"][0]["pending"] == 0)
                .then_some(())
        },
    );
    f.shutdown();
}

provider_matrix!(
    caller_value_errors_remain_actionable,
    caller_value_errors_case
);
fn caller_value_errors_case(provider: Provider) {
    let comb = CombService::new();
    comb.request(Operation::Put(comb_protocol::service::api::PutRequest {
        slug: "concepts/input".parse().unwrap(),
        markdown: "---\ntitle: Input\n---\nOriginal".into(),
        expected_hash: None,
        request_id: "input-original".parse().unwrap(),
        run: None,
    }));
    let path = comb.directory.path().join("concepts/input.md");
    let original = fs::read(&path).unwrap();
    let mut f = Harness::builder(provider).build();
    f.edit_config(|config| {
        config["comb"] = json!({"credential_file":comb.credential});
    });
    f.start();
    let (actions, expected) = invalid_value_actions();
    let credential = fs::read(&comb.credential).unwrap();
    let mut wrong: Value = serde_json::from_slice(&credential).unwrap();
    wrong["credential"]["secret"] = json!("incorrect");
    let mut agent = None;
    for (round, (edited, bad_secret)) in
        [(false, false), (false, true), (true, false), (true, true)]
            .into_iter()
            .enumerate()
    {
        let contents = if edited {
            b"unparseable external page".as_slice()
        } else {
            original.as_slice()
        };
        fs::write(&path, contents).unwrap();
        fs::write(
            &comb.credential,
            if bad_secret {
                serde_json::to_vec(&wrong).unwrap()
            } else {
                credential.clone()
            },
        )
        .unwrap();
        if let Some(id) = &agent {
            f.ok(json!({"op":"send","target":id,"body":body(json!(actions))}));
        } else {
            agent = Some(spawn(&f, provider, false, json!(actions)));
        }
        let replies = results(&f, agent.as_deref().unwrap(), round + 1);
        for (reply, error) in replies.iter().zip(&expected) {
            let reply = support::reply::Failure::parse(reply.clone());
            assert_eq!(reply.error.code, error.code.as_str(), "{reply:?}");
            assert_eq!(reply.error.message, error.message.as_ref(), "{reply:?}");
            if let Some(input) = error.input() {
                assert_eq!(reply.error.field.as_deref().unwrap(), input.field());
                assert_eq!(reply.error.expected.as_deref().unwrap(), input.expected());
                assert_eq!(reply.error.examples.as_ref().unwrap().len(), 1);
            }
            assert!(reply.error.request_id.is_none(), "{reply:?}");
            assert_eq!(reply.error.file_committed, Some(false));
            assert!(
                !serde_json::to_string(&reply)
                    .unwrap()
                    .contains("BAD SECRET INPUT")
            );
        }
        assert_eq!(replies.len(), expected.len());
        assert_eq!(fs::read(&path).unwrap(), contents);
    }
    f.shutdown();
}

mod value_cases;
use value_cases::invalid_value_actions;

mod collector_scheduling;

fn configure_curator(f: &mut Harness, comb: &CombService, provider: Provider) {
    f.edit_config(|config| {
        config["comb"] = json!({"credential_file":comb.credential});
        let base = if provider == Provider::Claude {
            "claude"
        } else {
            "implementor"
        };
        config["roles"]["curator"] = config["roles"][base].clone();
        config["roles"]["curator"]["lifetime"] = json!("independent");
        let reviewer = if provider == Provider::Claude {
            "claude-reviewer"
        } else {
            "reviewer"
        };
        config["roles"][reviewer]["comb"] =
            json!({"credential_file":comb.credential,"reviewer":true});
    });
}

fn configure_native_comb(f: &mut Harness, comb: &CombService) {
    f.edit_config(|config| {
        config["comb"] = json!({"credential_file":comb.credential});
        config["runtime"] = json!({"max_loaded":8,"max_providers":2,
            "sessions_per_codex_process":4,"max_starting":2,"max_active_turns":8});
        for role in ["reviewer", "claude-reviewer"] {
            if !config["roles"][role].is_null() {
                config["roles"][role]["comb"] = json!({
                "credential_file":comb.credential,"reviewer":true});
            }
        }
    });
}
