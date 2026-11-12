use super::*;

async fn seed(human: &Connection) -> usize {
    let scenario: Value =
        serde_json::from_str(include_str!("../../../../compat/synthetic-0/scenario.json")).unwrap();
    let pages = scenario["pages"].as_object().unwrap();
    let writer = issue(
        human,
        Grant::Client {
            name: "corpus-writer".into(),
            reviewer: false,
        },
    )
    .await;
    for (name, page) in pages {
        let connection = if page["unverified"].as_bool().unwrap() {
            &writer
        } else {
            human
        };
        let operation = serde_json::from_value(json!({
            "op":"put", "slug":page["slug"], "markdown":page["markdown"],
            "request_id":format!("corpus-{name}")
        }))
        .unwrap();
        call(connection, &operation, None).await.unwrap();
    }
    let note = pages["note"]["slug"].as_str().unwrap();
    let target = pages["trusted"]["slug"].as_str().unwrap();
    let note_hash = get(human, note).await["hash"].clone();
    let target_hash = get(human, target).await["hash"].clone();
    let operation = serde_json::from_value(json!({
        "op":"consolidate", "request_id":"corpus-fold", "run":"synthetic-release",
        "request":{
            "note":note, "target":target, "note_hash":note_hash, "target_hash":target_hash,
            "summary":scenario["summary"]
        }
    }))
    .unwrap();
    call(human, &operation, None).await.unwrap();
    pages.len()
}

#[test]
fn synthetic_release_service_and_mcp_transcripts() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../compat/synthetic-0/transcripts.json"
    ))
    .unwrap();
    let (_dir, server, human) = setup(None);
    runtime().block_on(async {
        let running = Running::start(server);
        let count = seed(&human).await;
        let listing = Operation::List { request: comb_protocol::request::List::default() };
        let before = call(&human, &listing, None).await.unwrap();
        assert_eq!(before.as_array().unwrap().len(), count, "listing must cover every seeded page");
        for case in fixture["service"].as_array().unwrap() {
            let mut wire = BufReader::new(UnixStream::connect(&human.socket).await.unwrap());
            let request = json!({"protocol":case["protocol"],"credential":human.credential,"claim":null,"operation":case["operation"]});
            framing::write(wire.get_mut(), &request, framing::REQUEST_BYTES).await.unwrap();
            let bytes = tokio::time::timeout(Duration::from_secs(5), framing::read(&mut wire, framing::RESPONSE_BYTES)).await.unwrap().unwrap().unwrap();
            let reply: Value = serde_json::from_slice(&bytes).unwrap();
            for (pointer, expected) in case["expect"].as_object().unwrap() {
                assert_eq!(reply.pointer(pointer), Some(expected), "{reply}");
            }
        }
        // Skewed capture never reached the writer.
        let listed = call(&human, &listing, None).await.unwrap();
        assert_eq!(listed, before, "skewed capture changed the seeded root");
        for case in fixture["mcp"].as_array().unwrap() {
            let (client, server) = UnixStream::pair().unwrap();
            let (input, output) = server.into_split();
            let proxy = tokio::spawn(super::super::mcp::run(human.clone(), BufReader::new(input), output));
            let mut wire = BufReader::new(client);
            for request in [
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":case["offer"],"clientInfo":{},"capabilities":{}}}),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                json!({"jsonrpc":"2.0","id":2,"method":"ping"}),
            ] {
                framing::write(wire.get_mut(), &request, framing::REQUEST_BYTES).await.unwrap();
            }
            for id in 1..=2 {
                let bytes = tokio::time::timeout(Duration::from_secs(5), framing::read(&mut wire, framing::MCP_BYTES)).await.unwrap().unwrap().unwrap();
                let reply: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(reply["id"], id);
                if id == 1 { assert_eq!(reply["result"]["protocolVersion"], case["selected"]); }
                else { assert_eq!(reply["result"], json!({})); }
            }
            drop(wire);
            proxy.await.unwrap().unwrap();
        }
        running.finish().await;
    });
}
