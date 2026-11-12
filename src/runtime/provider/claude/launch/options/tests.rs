use super::*;
use serde_json::json;

#[test]
fn numeric_launch_fields_match_the_previous_unsigned_value_projection() {
    for value in [
        json!(null),
        json!(-1),
        json!(0),
        json!(u64::MAX),
        json!(1.0),
        json!("1"),
        json!(true),
        json!([]),
        json!({}),
    ] {
        let values = [
            "claude.max_output_tokens",
            "claude.max_mcp_output_tokens",
            "claude.auto_compact_window",
        ]
        .map(|name| (name.to_owned(), value.clone()))
        .into();
        let options = Options::read(&values);
        assert_eq!(options.output_tokens, value.as_u64());
        assert_eq!(options.mcp_tokens, value.as_u64());
        assert_eq!(options.compact_window, value.as_u64());
    }
    let options = Options::read(&OptionMap::default());
    assert!(options.output_tokens.is_none());
    assert!(options.mcp_tokens.is_none());
    assert!(options.compact_window.is_none());
}

#[test]
fn thinking_counts_modes_and_wrong_shapes_preserve_the_old_meaning() {
    for (value, tokens, enabled) in [
        (json!(0), Some(0), Some(true)),
        (json!(u64::MAX), Some(u64::MAX), Some(true)),
        (json!("on"), None, Some(true)),
        (json!("off"), None, Some(false)),
        (json!("ON"), None, None),
        (json!("0"), None, None),
        (json!(-1), None, None),
        (json!(1.0), None, None),
        (json!(true), None, None),
        (json!(null), None, None),
        (json!([]), None, None),
        (json!({}), None, None),
    ] {
        let options = Options::read(&[("thinking".into(), value)].into());
        assert_eq!(options.thinking.tokens(), tokens);
        assert_eq!(options.thinking.enabled(), enabled);
    }
}

#[test]
fn malformed_neighbors_unknown_fields_and_duplicates_do_not_erase_valid_options() {
    let values: OptionMap = serde_json::from_str(concat!(
        "{\"thinking\":\"off\",\"claude.max_output_tokens\":[],",
        "\"claude.max_mcp_output_tokens\":7,\"claude.auto_compact_window\":null,",
        "\"thinking\":0,\"extra\":{\"nested\":[null,1]}}"
    ))
    .unwrap();
    let options = Options::read(&values);
    assert!(options.output_tokens.is_none());
    assert_eq!(options.mcp_tokens, Some(7));
    assert!(options.compact_window.is_none());
    assert_eq!(options.thinking.tokens(), Some(0));
}

fn environment(
    command: &std::process::Command,
) -> std::collections::BTreeMap<String, Option<String>> {
    command
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_str().unwrap().to_owned(),
                value.map(|value| value.to_str().unwrap().to_owned()),
            )
        })
        .collect()
}

#[test]
fn environment_keeps_unsigned_precision_and_does_not_remove_preexisting_values_on_absence() {
    let keys = [
        "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
        "MAX_MCP_OUTPUT_TOKENS",
        "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING",
        "MAX_THINKING_TOKENS",
    ];
    let mut unchanged = std::process::Command::new("unused");
    for key in keys {
        unchanged.env(key, "retained");
    }
    super::super::SessionConfig::configure_environment(&mut unchanged, &Options::default());
    assert_eq!(
        environment(&unchanged),
        keys.map(|key| (key.into(), Some("retained".into()))).into()
    );
    let options = Options::read(
        &[
            ("claude.max_output_tokens".into(), json!(u64::MAX)),
            ("claude.max_mcp_output_tokens".into(), json!(0)),
            ("thinking".into(), json!(0)),
        ]
        .into(),
    );
    let mut command = std::process::Command::new("unused");
    super::super::SessionConfig::configure_environment(&mut command, &options);
    assert_eq!(
        environment(&command),
        keys.into_iter()
            .zip([u64::MAX.to_string(), "0".into(), "1".into(), "0".into()])
            .map(|(key, value)| (key.into(), Some(value)))
            .collect()
    );
}
