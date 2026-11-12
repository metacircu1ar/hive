use super::{parse, tests::rejected};
use std::ffi::OsString;

#[test]
fn model_use_accepts_only_none_and_qwen3_local() {
    assert!(parse(&["comb", "model", "use", "none"].map(OsString::from)).is_ok());
    assert!(parse(&["comb", "model", "use", "qwen3-local"].map(OsString::from)).is_ok());
    assert!(
        parse(
            &[
                "comb",
                "model",
                "use",
                "qwen3-local",
                "--llama-server",
                "/private/llama-server",
                "--llama-server-arg",
                "--n-gpu-layers",
                "--llama-server-arg=99",
            ]
            .map(OsString::from)
        )
        .is_ok()
    );
    for name in ["", "default", "PRIVATE/name", "x"] {
        let output = rejected(&["model", "use", name]);
        assert_eq!(output.exit_code, 2);
        let json: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(json["error"]["field"], "NAME");
        assert_eq!(json["error"]["code"], "invalid_value");
        let expected = json["error"]["expected"].as_str().unwrap();
        assert!(
            expected.starts_with("none (text search only) or qwen3-local"),
            "{expected}"
        );
        assert!(!json["error"]["examples"].as_array().unwrap().is_empty());
        assert!(!output.stdout.contains("PRIVATE"));
        assert!(!output.stderr.contains("PRIVATE"));
    }
}

#[test]
fn server_arguments_need_a_user_server_and_a_value() {
    let output = rejected(&[
        "model",
        "use",
        "qwen3-local",
        "--llama-server-arg",
        "--verbose",
    ]);
    assert_eq!(output.exit_code, 2);
    let output = rejected(&[
        "model",
        "use",
        "qwen3-local",
        "--llama-server",
        "/private/llama-server",
        "--llama-server-arg=",
    ]);
    assert_eq!(output.exit_code, 2);
    let json: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(json["error"]["field"], "--llama-server-arg");
    assert!(!output.stdout.contains("/private/llama-server"));
}
