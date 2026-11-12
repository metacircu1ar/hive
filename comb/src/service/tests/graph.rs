//! Functional graph queries through authenticated socket and stdio MCP adapters.
use super::*;

#[test]
fn graph_query_matches_across_socket_and_mcp_without_a_model_or_reviewer_grant() {
    let (_dir, server, human) = setup(None);
    runtime().block_on(async {
        let running = Running::start(server);
        let writer = issue(&human, Grant::Client { name: "writer".into(), reviewer: false }).await;
        call(&writer, &put("concepts/a", "## Relations\n- [[depends_on::concepts/b]]", "a"), None).await.unwrap();
        call(&writer, &put("concepts/b", "Target", "b"), None).await.unwrap();
        let args = json!({"anchor":"concepts/a","predicates":["depends_on"]});
        let op = super::super::mcp::operation("comb_graph_query", args.clone(), None).unwrap();
        let direct = call(&writer, &op, None).await.unwrap();
        assert_eq!(direct["complete"], true);
        assert_eq!(direct["relations"][0]["declaration"]["unverified"], true);
        let (client, server) = UnixStream::pair().unwrap();
        let (input, output) = server.into_split();
        let proxy = tokio::spawn(super::super::mcp::run(writer, BufReader::new(input), output));
        let mut wire = BufReader::new(client);
        for frame in [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{},"capabilities":{}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"comb_graph_query","arguments":args}}),
        ] { framing::write(wire.get_mut(), &frame, framing::REQUEST_BYTES).await.unwrap(); }
        let _initialized = framing::read(&mut wire, framing::MCP_BYTES).await.unwrap().unwrap();
        let reply: Value = serde_json::from_slice(&framing::read(&mut wire, framing::MCP_BYTES).await.unwrap().unwrap()).unwrap();
        assert_eq!(reply["result"]["isError"], false);
        let graph: Value = serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(graph, direct);
        drop(wire);
        proxy.await.unwrap().unwrap();
        running.finish().await;
    });
}
