//! Http contracts regressions.
use super::*;

#[test]
fn http_checks_indices_count_dimensions_zero_norm_and_accepts_shuffled_items() {
    let cases = [
        (
            r#"{"data":[{"index":1,"embedding":[0,2]},{"index":0,"embedding":[3,0]}]}"#,
            true,
        ),
        (
            r#"{"data":[{"index":0,"embedding":[1,0]},{"index":0,"embedding":[1,0]}]}"#,
            false,
        ),
        (r#"{"data":[{"index":0,"embedding":[1,0]}]}"#, false),
        (
            r#"{"data":[{"index":2,"embedding":[1,0]},{"index":0,"embedding":[1,0]}]}"#,
            false,
        ),
        (
            r#"{"data":[{"index":0,"embedding":[0,0]},{"index":1,"embedding":[1,0]}]}"#,
            false,
        ),
        (
            r#"{"data":[{"index":0,"embedding":[1]},{"index":1,"embedding":[1,0]}]}"#,
            false,
        ),
    ];
    for (body, valid) in cases {
        let (mut engine, server) = http_reply(body.into(), "", |_| {});
        let response = engine.embed(&["one".into(), "two".into()]);
        assert_eq!(response.is_ok(), valid);
        if valid {
            let values = response.unwrap();
            assert_eq!(values[0].as_ref().unwrap(), &vec![1.0, 0.0]);
        }
        let sent: serde_json::Value = serde_json::from_str(&server.join().unwrap()).unwrap();
        assert_eq!(sent["input"], serde_json::json!(["one", "two"]));
        assert!(sent.get("dimensions").is_none());
    }
}

#[test]
fn static_http_identity_matches_loaded_engine_without_using_endpoint() {
    let config: HttpConfig = serde_json::from_value(serde_json::json!({
        "model":"test", "revision":"fixed", "url":"http://127.0.0.1:1/v1/embeddings",
        "dimensions":3, "query_prefix":"query: ", "document_prefix":"passage: ",
        "request_dimensions":true
    }))
    .unwrap();
    let identity = Engine::http_identity(config.clone()).unwrap();
    let engine = Engine::load(Config::Http(config)).unwrap();
    assert_eq!(identity, *engine.space());
}
