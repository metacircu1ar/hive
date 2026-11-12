//! Session configuration regressions.
use super::*;

#[test]
fn models_without_effort_use_none_and_omit_the_native_flag() {
    let launcher = crate::launch::test_cache().unwrap();
    let init:Initialization=serde_json::from_value(json!({"account":{"subscriptionType":"Team","apiProvider":"firstParty"},
        "models":[{"value":"simple","resolvedModel":"simple-1","displayName":"Simple","description":"test"}]})).unwrap();
    assert_eq!(
        catalog(&init).unwrap()["data"][0]["supportedReasoningEfforts"][0]["reasoningEffort"],
        "none"
    );
    let dir = tempfile::tempdir().unwrap();
    let config = SessionConfig {
        model: "simple".into(),
        effort: "none".into(),
        ..SessionConfig::for_test(
            Uuid::new_v4().to_string(),
            dir.path().canonicalize().unwrap(),
        )
    };
    let mut command = Command::new("claude");
    let guard = config.configure(&mut command, &launcher).unwrap();
    assert!(!command.get_args().any(|arg| arg == "--effort"));
    let args: Vec<_> = command.get_args().collect();
    let settings = args
        .windows(2)
        .find(|pair| pair[0] == "--settings")
        .unwrap()[1];
    let expected = json!({"sandbox":{
        "enabled":true,"failIfUnavailable":true,"autoAllowBashIfSandboxed":true,
        "allowUnsandboxedCommands":false,"excludedCommands":[],
        "network":{"allowUnixSockets":[guard.directory().join("socket")],"allowAllUnixSockets":false},
        "filesystem":{"denyWrite":["/"]}},
        "permissions":{"deny":["TaskOutput","TaskStop","CronCreate","CronDelete","CronList"]},
        "autoMemoryEnabled":false,"autoDreamEnabled":false});
    assert_eq!(settings.to_str().unwrap(), expected.to_string());
}

#[test]
fn optional_session_settings_and_generated_environment_reset_by_absence() {
    let launcher = crate::launch::test_cache().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut configured = SessionConfig::for_test(
        Uuid::new_v4().to_string(),
        dir.path().canonicalize().unwrap(),
    );
    std::sync::Arc::make_mut(&mut configured.options).values = [
        ("thinking".into(), json!(4096)),
        ("claude.auto_compact_window".into(), json!(200_000)),
        ("claude.max_output_tokens".into(), json!(50000)),
        ("claude.max_mcp_output_tokens".into(), json!(4000)),
    ]
    .into_iter()
    .collect();
    let mut command = Command::new("claude");
    configured.configure(&mut command, &launcher).unwrap();
    let args: Vec<_> = command.get_args().collect();
    let settings: Value = serde_json::from_str(
        args.windows(2)
            .find(|pair| pair[0] == "--settings")
            .unwrap()[1]
            .to_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(settings["alwaysThinkingEnabled"], true);
    assert_eq!(settings["autoCompactWindow"], 200_000);
    assert_eq!(settings["autoDreamEnabled"], false);
    assert_session_environment(&command);

    let mut reset = Command::new("claude");
    SessionConfig::for_test(
        Uuid::new_v4().to_string(),
        dir.path().canonicalize().unwrap(),
    )
    .configure(&mut reset, &launcher)
    .unwrap();
    let reset_args: Vec<_> = reset.get_args().collect();
    let reset_settings: Value = serde_json::from_str(
        reset_args
            .windows(2)
            .find(|pair| pair[0] == "--settings")
            .unwrap()[1]
            .to_str()
            .unwrap(),
    )
    .unwrap();
    assert!(reset_settings.get("alwaysThinkingEnabled").is_none());
    assert!(reset_settings.get("autoCompactWindow").is_none());
    assert_eq!(reset_settings["autoDreamEnabled"], false);
    assert!(reset.get_envs().all(|(key, _)| {
        !matches!(
            key.to_str(),
            Some(
                "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING"
                    | "MAX_THINKING_TOKENS"
                    | "CLAUDE_CODE_MAX_OUTPUT_TOKENS"
                    | "MAX_MCP_OUTPUT_TOKENS"
            )
        )
    }));
}

fn assert_session_environment(command: &Command) {
    let environment: std::collections::BTreeMap<_, _> = command
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect();
    assert_eq!(
        environment["CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING"].as_deref(),
        Some("1")
    );
    assert_eq!(environment["MAX_THINKING_TOKENS"].as_deref(), Some("4096"));
    assert_eq!(
        environment["CLAUDE_CODE_MAX_OUTPUT_TOKENS"].as_deref(),
        Some("50000")
    );
    assert_eq!(
        environment["MAX_MCP_OUTPUT_TOKENS"].as_deref(),
        Some("4000")
    );
}
