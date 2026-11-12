use super::*;

#[test]
fn admitted_vector_searches_keep_the_query_owner_during_shutdown_drain() {
    let engine = fake(Duration::ZERO).hold_on("q:");
    let release = ModelRelease(engine.gate());
    let query_calls = engine.query_calls();
    let (_dir, server, human) = setup(Some(Box::new(engine)));
    let state = server.state.clone();
    runtime().block_on(async {
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let serving = tokio::spawn(server.run(async {
            stopped.await.unwrap();
        }));
        call(
            &human,
            &put("concepts/page", "Fact for search", "create"),
            None,
        )
        .await
        .unwrap();
        until(|| coverage(&state).embedded > 0).await;
        let spawn_query = || {
            let human = human.clone();
            tokio::spawn(async move { call(&human, &query("holdquery", Mode::Vector), None).await })
        };
        let first = spawn_query();
        until(|| query_calls.load(Ordering::SeqCst) > 0).await;
        // First batch is inside the blocked model; the second remains queued.
        let second = spawn_query();
        until(|| state.queries.slots.available_permits() == limits::QUERY_SLOTS - 2).await;
        stop.send(()).unwrap();
        until(|| std::os::unix::net::UnixStream::connect(&human.socket).is_err()).await;
        drop(release);
        for result in [first, second] {
            let response = result.await.unwrap().unwrap();
            assert_eq!(response["mode"], "vector");
            assert!(
                response["results"]
                    .as_array()
                    .is_some_and(|rows| !rows.is_empty())
            );
        }
        serving.await.unwrap().unwrap();
    });
}
