use super::*;
use std::ffi::OsString;

fn strings(argv: &[OsString]) -> Vec<String> {
    argv.iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect()
}
fn value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
    let index = argv.iter().position(|value| value == flag)?;
    argv.get(index + 1).map(String::as_str)
}

#[test]
fn bundled_metal_and_cpu_and_user_binaries_share_the_validated_argv() {
    let model = Path::new("/private/models/model.gguf");
    let metal = strings(&argv(model, 8081, Device::Metal, &[]));
    let cpu = strings(&argv(model, 8081, Device::Cpu, &[]));
    let own = strings(&argv(
        model,
        8081,
        Device::Own,
        &["--n-gpu-layers".into(), "99".into()],
    ));
    for argv in [&metal, &cpu, &own] {
        assert_eq!(value(argv, "--model"), Some("/private/models/model.gguf"));
        assert_eq!(value(argv, "--pooling"), Some("last"));
        assert_eq!(value(argv, "--embd-normalize"), Some("2"));
        assert_eq!(value(argv, "--flash-attn"), Some("on"));
        assert_eq!(value(argv, "--parallel"), Some("4"));
        assert_eq!(value(argv, "--host"), Some("127.0.0.1"));
        assert_eq!(value(argv, "--port"), Some("8081"));
        assert!(argv.iter().any(|value| value == "--no-webui"));
        assert!(argv.iter().any(|value| value == "--embedding"));
    }
    assert_eq!(value(&metal, "--device"), Some("MTL0"));
    assert_eq!(value(&metal, "--ctx-size"), Some("32768"));
    assert_eq!(value(&metal, "--ubatch-size"), Some("8192"));
    assert_eq!(value(&cpu, "--device"), Some("none"));
    assert_eq!(value(&cpu, "--n-gpu-layers"), Some("0"));
    let slot = CPU_SLOT_TOKENS.to_string();
    assert_eq!(value(&cpu, "--ubatch-size"), Some(slot.as_str()));
    // A user binary picks its own device; its arguments come last.
    assert_eq!(value(&own, "--device"), None);
    assert_eq!(&own[own.len() - 2..], ["--n-gpu-layers", "99"]);
    assert_eq!(value(&own, "--ctx-size"), Some("32768"));
}

#[test]
fn configuration_is_loopback_4096_dimensional_with_device_deadlines() {
    let pin = Pin::parse(&pin_text(&"d".repeat(40), &"f".repeat(64), 5)).unwrap();
    let metal = config(&pin, Device::Metal, 8081);
    assert_eq!(metal.url, "http://127.0.0.1:8081/v1/embeddings");
    assert_eq!(metal.dimensions, DIMENSIONS);
    assert_eq!(metal.model, ALIAS);
    assert_eq!(metal.revision, pin.revision());
    assert_eq!(metal.timeout_seconds, TIMEOUT_SECONDS);
    assert_eq!(metal.query_prefix, QUERY_PREFIX);
    assert!(metal.api_key_env.is_none() && metal.tokenizer.is_none());
    assert_eq!(metal.max_request_bytes, None);
    assert_eq!(config(&pin, Device::Own, 8081).max_request_bytes, None);
    assert_eq!(
        config(&pin, Device::Cpu, 8081).max_request_bytes,
        Some(CPU_REQUEST_BYTES)
    );
    assert_eq!(
        config(&pin, Device::Cpu, 8081).timeout_seconds,
        CPU_TIMEOUT_SECONDS
    );
    assert!(crate::embedding::Engine::http_identity(metal.clone()).is_ok());
    assert_eq!(argv::port(&metal), Some(8081));
    for url in [
        "http://127.0.0.1:08081/v1/embeddings",
        "http://localhost:8081/v1/embeddings",
        "http://127.0.0.1:0/v1/embeddings",
        "http://127.0.0.1:8081/embeddings",
    ] {
        let mut other = metal.clone();
        other.url = url.into();
        assert_eq!(argv::port(&other), None, "{url}");
    }
}

#[test]
fn stored_marker_shape_and_validation() {
    let pin = Pin::parse(&pin_text(&"d".repeat(40), &"f".repeat(64), 5)).unwrap();
    let http = config(&pin, Device::Own, 8081);
    let user = ManagedServer {
        binary: Binary::Path("/opt/llama/bin/llama-server".into()),
        model: "/private/cache/model.gguf".into(),
        arguments: vec!["--n-gpu-layers".into(), "99".into()],
    };
    assert_eq!(user.validate(Some(&http)).unwrap(), 8081);
    let json = serde_json::to_value(&user).unwrap();
    assert_eq!(json["binary"]["path"], "/opt/llama/bin/llama-server");
    let bundled = ManagedServer {
        binary: Binary::Bundled,
        model: "/private/cache/model.gguf".into(),
        arguments: vec![],
    };
    assert_eq!(serde_json::to_value(&bundled).unwrap()["binary"], "bundled");
    for invalid in [
        ManagedServer {
            arguments: vec!["--verbose".into()],
            ..bundled.clone()
        },
        ManagedServer {
            model: "relative/model.gguf".into(),
            ..bundled.clone()
        },
        ManagedServer {
            binary: Binary::Path("llama-server".into()),
            ..bundled.clone()
        },
        ManagedServer {
            arguments: vec![String::new()],
            ..user.clone()
        },
    ] {
        assert_eq!(
            invalid.validate(Some(&http)).unwrap_err().code,
            ErrorCode::InvalidEmbeddingConfig
        );
    }
    assert!(bundled.validate(None).is_err());
    let unknown = serde_json::json!({
        "binary": "bundled", "model": "/m.gguf", "arguments": [], "port": 1
    });
    assert!(serde_json::from_value::<ManagedServer>(unknown).is_err());
}
