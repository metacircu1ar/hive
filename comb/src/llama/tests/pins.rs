use super::*;

#[test]
fn complete_pin_parses_and_placeholders_or_extra_fields_are_refused() {
    let pin = Pin::parse(&pin_text(&"d".repeat(40), &"f".repeat(64), 5)).unwrap();
    assert_eq!(pin.commit, COMMIT);
    assert_eq!(pin.revision(), format!("q8_0-{}", "f".repeat(64)));
    assert_eq!(
        pin.url(),
        format!(
            "https://huggingface.co/Example/model/resolve/{}/model.gguf",
            "e".repeat(40)
        )
    );
    for text in [
        pin_text("UNVERIFIED: record the tree", &"f".repeat(64), 5),
        pin_text(&"d".repeat(40), &"F".repeat(64), 5),
        pin_text(&"d".repeat(40), &"f".repeat(64), 0),
        pin_text(&"d".repeat(40), &"f".repeat(63), 5),
    ] {
        let error = Pin::parse(&text).unwrap_err();
        assert_eq!(error.code, ErrorCode::EmbeddingModel);
        assert_eq!(error.message, crate::Error::llama_pin_unverified().message);
    }
    let mut extra: serde_json::Value =
        serde_json::from_str(&pin_text(&"d".repeat(40), &"f".repeat(64), 5)).unwrap();
    extra["model"]["mirror"] = "https://example.invalid".into();
    assert!(Pin::parse(&extra.to_string()).is_err());
}

#[test]
fn shipped_pin_is_complete() {
    // Fails until the verified llama.cpp tree ID replaces its placeholder.
    let pin = Pin::shipped().unwrap();
    assert_eq!(pin.model.file, "Qwen3-Embedding-8B-Q8_0.gguf");
    assert_eq!(pin.model.bytes, 8_047_105_824);
}

#[test]
fn shipped_example_configuration_matches_the_generated_metal_identity() {
    let pin = Pin::shipped().unwrap();
    let example: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/qwen3-embedding.json")).unwrap();
    let generated = serde_json::to_value(crate::embedding::Config::Http(config(
        &pin,
        Device::Metal,
        PREFERRED_PORT,
    )))
    .unwrap();
    for field in [
        "model",
        "revision",
        "url",
        "dimensions",
        "query_prefix",
        "document_prefix",
    ] {
        assert_eq!(example[field], generated[field], "{field}");
    }
}

#[test]
fn tested_versions_are_exact_commit_prefixes() {
    let pin = Pin::parse(&pin_text(&"d".repeat(40), &"f".repeat(64), 5)).unwrap();
    assert!(pin.tested(&COMMIT[..7]));
    assert!(pin.tested(COMMIT));
    assert!(!pin.tested(&COMMIT[..6]));
    assert!(!pin.tested("fedcba9"));
    assert_eq!(
        probe::reported_commit(
            "ggml init\nversion: 0.6.0-dev (build 7012, commit 0123456)\nbuilt with cc"
        ),
        Some("0123456")
    );
    assert_eq!(
        probe::reported_commit("version: 0.6.0-dev (build 1, commit XYZ)"),
        None
    );
    assert_eq!(probe::reported_commit("llama-server b1234"), None);
}
