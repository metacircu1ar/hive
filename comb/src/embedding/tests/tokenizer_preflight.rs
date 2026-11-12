//! Tokenizer preflight regressions.
use super::*;

#[test]
fn request_byte_budget_counts_prefixes_and_is_retained_by_query_clones() -> Result<()> {
    let config = serde_json::from_value(serde_json::json!({
        "backend":"http","model":"fixture","revision":"fixed","dimensions":2,
        "url":"http://127.0.0.1:9/v1/embeddings","query_prefix":"query: ",
        "max_request_bytes":64
    }))?;
    let mut engine = Engine::load(config)?;
    let rows = engine.embed(&["é".repeat(33)])?;
    let Err(error) = &rows[0] else {
        panic!("expected byte-budget rejection")
    };
    assert_eq!(error.code, crate::ErrorCode::EmbeddingInputTooLarge);
    let mut query = engine.query_engine().unwrap();
    let (_root, mut comb, _) = setup();
    let lease = comb.activate_embeddings(engine.space(), true)?;
    // The raw query fits in 64 bytes; its seven-byte prefix takes it over.
    let Err(error) = run_query(query.as_mut(), lease, &"é".repeat(29)) else {
        panic!("expected prefixed query rejection before HTTP");
    };
    assert_eq!(error.code, crate::ErrorCode::EmbeddingInputTooLarge);
    assert!(error.message.contains("64 UTF-8 bytes"));
    Ok(())
}

#[test]
fn http_tokenizer_assets_can_exceed_sixteen_mib_but_remain_bounded() {
    let root = tempfile::tempdir().unwrap();
    let tokenizer = root.path().join("tokenizer.json");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/http-tokenizer/tokenizer.json"),
        &tokenizer,
    )
    .unwrap();
    let mut bytes = std::fs::read(&tokenizer).unwrap();
    bytes.resize(17 * 1024 * 1024, b' ');
    std::fs::write(&tokenizer, bytes).unwrap();
    let json = serde_json::json!({"backend":"http","model":"m","revision":"r","url":"http://127.0.0.1:9/v1/embeddings","dimensions":2,"tokenizer":tokenizer,"max_tokens":16});
    let load = || Engine::load(serde_json::from_value(json.clone()).unwrap());
    load().unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&tokenizer)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    assert_eq!(
        load().err().unwrap().code,
        crate::ErrorCode::EmbeddingAssetSize
    );
}

#[test]
fn http_dimensions_opt_in_and_token_preflight_apply_to_query_clones() {
    let tokenizer = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/http-tokenizer/tokenizer.json");
    let (mut engine, server) = http_reply(
        r#"{"data":[{"index":0,"embedding":[1,0]}]}"#.into(),
        "",
        |config| {
            let Config::Http(HttpConfig {
                request_dimensions,
                tokenizer: path,
                max_tokens,
                query_prefix,
                ..
            }) = config;
            *request_dimensions = true;
            *path = Some(tokenizer);
            *max_tokens = Some(16);
            *query_prefix = "query: ".into();
        },
    );
    assert_eq!(
        engine.embed(&["cat ".repeat(17)]).unwrap()[0]
            .as_ref()
            .err()
            .unwrap()
            .code,
        crate::ErrorCode::EmbeddingInputTooLarge
    );
    let mut query_engine = engine.query_engine().unwrap();
    let (_root, mut comb, _) = setup();
    let lease = comb.activate_embeddings(engine.space(), true).unwrap();
    let error = run_query(query_engine.as_mut(), lease.clone(), &"cat ".repeat(15))
        .err()
        .unwrap();
    assert_eq!(error.code, crate::ErrorCode::EmbeddingInputTooLarge);
    assert!(error.message.contains("16 tokens"));
    assert!(run_query(query_engine.as_mut(), lease, "cat").is_ok());
    let sent: serde_json::Value = serde_json::from_str(&server.join().unwrap()).unwrap();
    assert_eq!(sent["dimensions"], 2);
    assert_eq!(sent["input"], serde_json::json!(["query: cat"]));
}

#[test]
fn http_preflight_is_bounded_and_policy_changes_require_explicit_replacement() {
    let root = tempfile::tempdir().unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/http-tokenizer/tokenizer.json");
    let tokenizer = root.path().join("tokenizer.json");
    std::fs::copy(fixture, &tokenizer).unwrap();
    let mut json = serde_json::json!({"backend":"http","model":"m","revision":"r","url":"http://127.0.0.1:9/v1/embeddings","dimensions":2,"tokenizer":tokenizer,"max_tokens":16});
    let load = |value: serde_json::Value| Engine::load(serde_json::from_value(value).unwrap());
    let a = load(json.clone()).unwrap();
    let (_root, mut comb, _) = setup();
    comb.activate_embeddings(a.space(), true).unwrap();
    for (key, value) in [
        ("max_tokens", serde_json::json!(32)),
        ("request_dimensions", serde_json::json!(true)),
    ] {
        let mut changed = json.clone();
        changed[key] = value;
        let b = load(changed).unwrap();
        assert_ne!(
            a.space().fingerprint().unwrap(),
            b.space().fingerprint().unwrap()
        );
        assert_eq!(
            comb.activate_embeddings(b.space(), false)
                .err()
                .unwrap()
                .code,
            crate::ErrorCode::EmbeddingReplaceRequired
        );
    }
    let mut bytes = std::fs::read(&tokenizer).unwrap();
    bytes.push(b' ');
    std::fs::write(&tokenizer, bytes).unwrap();
    assert_ne!(
        a.space().fingerprint().unwrap(),
        load(json.clone()).unwrap().space().fingerprint().unwrap()
    );
    json.as_object_mut().unwrap().remove("max_tokens");
    assert_eq!(
        load(json).err().unwrap().code,
        crate::ErrorCode::InvalidEmbeddingConfig
    );
}
