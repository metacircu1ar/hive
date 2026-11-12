use super::super::*;
use serde_json::json;

fn argument(command: &Command, name: &str) -> String {
    let args: Vec<_> = command.get_args().collect();
    args.windows(2).find(|pair| pair[0] == name).unwrap()[1]
        .to_str()
        .unwrap()
        .to_owned()
}

#[test]
fn configured_mcp_servers_and_sdk_override_keep_the_previous_exact_argument_bytes() {
    use std::os::unix::fs::PermissionsExt;
    let launcher = crate::launch::test_cache().unwrap();
    let workspace = crate::runtime::test_directory();
    let server = crate::config::McpServer {
        command: "command \"é".into(),
        args: vec!["line\narg".into()],
        env: [("VALUE".into(), "quoted\"value".into())].into(),
    };
    for servers in [
        crate::config::McpServers::default(),
        [
            ("zebra".into(), server.clone()),
            ("alpha".into(), server.clone()),
        ]
        .into(),
        [("hive".into(), server.clone()), ("other".into(), server)].into(),
    ] {
        let mut config = SessionConfig::for_test("child", PathBuf::from("/workspace"));
        config.mcp_servers = servers;
        let mut previous = api::value(&config.mcp_servers);
        previous["hive"] = json!({"type":"sdk","name":"hive"});
        let expected = json!({"mcpServers":previous}).to_string();
        let mut command = Command::new("unused");
        let guard = ShellGuard::create(workspace.path(), "", || launcher.executable()).unwrap();
        let (_guard, path) = mcp::publish(&config.mcp_servers, guard).unwrap();
        config.configure_arguments(&mut command, &path);
        assert_eq!(argument(&command, "--mcp-config"), path.to_str().unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            !command
                .get_args()
                .any(|arg| arg.to_string_lossy().contains("quoted\"value"))
        );
    }
}

// Frozen pre-S3c mapping; optional setting fields use the old Value projections.
fn previous_settings(
    config: &SessionConfig,
    guard: &ShellGuard,
    launcher: &LauncherCache,
) -> Value {
    let full = config.sandbox == Sandbox::FullAccess;
    let mut sandbox = json!({"enabled":!full,"failIfUnavailable":!full,
        "autoAllowBashIfSandboxed":!full,"allowUnsandboxedCommands":full,"excludedCommands":[]});
    if !full {
        sandbox["network"] = json!({"allowAllUnixSockets":false,
            "allowUnixSockets":[guard.directory().join("socket")]});
        sandbox["filesystem"] = api::value(config.restricted_filesystem(guard, launcher).unwrap());
    }
    let mut result = json!({"sandbox":sandbox,"permissions":{"deny":[
        "TaskOutput","TaskStop","CronCreate","CronDelete","CronList"]},
        "autoMemoryEnabled":false,"autoDreamEnabled":false});
    if let Some(thinking) = config.options.values.get("thinking").and_then(|value| {
        value.as_u64().map(|_| true).or_else(|| {
            value.as_str().and_then(|value| match value {
                "on" => Some(true),
                "off" => Some(false),
                _ => None,
            })
        })
    }) {
        result["alwaysThinkingEnabled"] = json!(thinking);
    }
    if let Some(window) = config
        .options
        .values
        .get("claude.auto_compact_window")
        .and_then(Value::as_u64)
    {
        result["autoCompactWindow"] = json!(window);
    }
    result
}

#[test]
fn typed_settings_keep_all_mode_and_optional_field_argument_bytes() {
    let launcher = crate::launch::test_cache().unwrap();
    let temp = crate::runtime::test_directory();
    let cwd = temp.path().canonicalize().unwrap();
    for sandbox in [
        Sandbox::ReadOnly,
        Sandbox::WorkspaceWrite,
        Sandbox::FullAccess,
    ] {
        for thinking in [
            json!(null),
            json!(0),
            json!(u64::MAX),
            json!("on"),
            json!("off"),
            json!(1.0),
        ] {
            let mut config = SessionConfig::for_test("child", cwd.clone());
            config.sandbox = sandbox;
            std::sync::Arc::make_mut(&mut config.options).values = [
                ("thinking".into(), thinking),
                ("claude.auto_compact_window".into(), json!(u64::MAX)),
            ]
            .into();
            let mut command = Command::new("unused");
            let guard = config.configure(&mut command, &launcher).unwrap();
            assert!(
                command
                    .get_envs()
                    .any(|(name, value)| name == "CLAUDE_CODE_SHELL_PREFIX" && value.is_none())
            );
            let expected = previous_settings(&config, &guard, &launcher).to_string();
            assert_eq!(argument(&command, "--settings"), expected);
        }
    }
}
