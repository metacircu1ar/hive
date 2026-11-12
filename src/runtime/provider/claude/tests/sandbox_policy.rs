//! Sandbox policy regressions.
use super::*;

#[test]
fn full_access_claude_allows_arbitrary_foreground_commands_and_external_edits() {
    for (tool, input) in [
        (
            "Bash",
            json!({"command":"cd /tmp && git init && git add . && git commit -m example; curl https://example.com", "dangerouslyDisableSandbox":true}),
        ),
        (
            "Write",
            json!({"file_path":"/somewhere/outside/workspace/file.txt","content":"hello"}),
        ),
        (
            "Edit",
            json!({"file_path":"/somewhere/outside/workspace/.git/config","old_string":"a","new_string":"b"}),
        ),
        (
            "NotebookEdit",
            json!({"notebook_path":"/somewhere/outside/workspace/book.ipynb"}),
        ),
    ] {
        let mut fixture = Fixture::with_extensions(None, Uuid::new_v4().to_string(), |config| {
            config.sandbox = Sandbox::FullAccess;
        });
        fixture.turn();
        let output = fixture.hook(tool, input);
        assert_eq!(output["permissionDecision"], "allow", "{tool}");
        if tool == "Bash" {
            assert_eq!(fixture.permission(output["updatedInput"].clone()), "allow");
            assert_eq!(fixture.permission(output["updatedInput"].clone()), "deny");
        }
    }
    for tool in [
        "TaskOutput",
        "TaskStop",
        "CronCreate",
        "CronDelete",
        "CronList",
    ] {
        let mut fixture = Fixture::with_extensions(None, Uuid::new_v4().to_string(), |config| {
            config.sandbox = Sandbox::FullAccess;
        });
        fixture.turn();
        assert_eq!(
            fixture.hook(tool, json!({}))["permissionDecision"],
            "deny",
            "{tool}"
        );
    }
}

#[test]
fn file_policy_rejects_case_variants_symlinks_and_dual_notebook_paths() {
    for path in [
        ".GIT/config",
        ".CLAUDE/settings.json",
        ".Codex/config.toml",
        ".huſ\u{212a}y/hook",
        ".proﬁle",
        ".mcp.jſon",
        "../escaped",
        "nested/.Git/hooks/x",
    ] {
        let mut f = Fixture::new();
        f.turn();
        let file = f.dir.path().canonicalize().unwrap().join(path);
        assert_eq!(
            f.hook("Write", json!({"file_path":file}))["permissionDecision"],
            "deny"
        );
    }
    let mut f = Fixture::new();
    f.turn();
    let good = f.dir.path().canonicalize().unwrap().join("good.ipynb");
    assert_eq!(
        f.hook(
            "NotebookEdit",
            json!({"file_path":good,"notebook_path":"/outside.ipynb"})
        )["permissionDecision"],
        "deny"
    );
    std::os::unix::fs::symlink("/tmp", f.dir.path().join("link")).unwrap();
    assert_eq!(
        f.hook(
            "Write",
            json!({"file_path":f.dir.path().canonicalize().unwrap().join("link/out")})
        )["permissionDecision"],
        "deny"
    );
    std::fs::create_dir(f.dir.path().join("ordinary")).unwrap();
    std::os::unix::fs::symlink("ordinary", f.dir.path().join(".git")).unwrap();
    assert_eq!(
        f.hook(
            "Write",
            json!({"file_path":f.dir.path().canonicalize().unwrap().join("ordinary/config")})
        )["permissionDecision"],
        "deny"
    );
}

#[test]
fn launch_rejects_protected_workspaces_and_gates_readonly_bash() {
    let launcher = crate::launch::test_cache().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut config = SessionConfig::for_test(
        Uuid::new_v4().to_string(),
        dir.path().canonicalize().unwrap(),
    );
    let mut command = std::process::Command::new("unused");
    let _guard = config.configure(&mut command, &launcher).unwrap();
    let args: Vec<_> = command.get_args().map(|s| s.to_str().unwrap()).collect();
    let tools = args[args.iter().position(|a| *a == "--tools").unwrap() + 1];
    // Read-only agents get Bash only for checked read-only git commands.
    assert_eq!(tools, "Bash,Read,Glob,Grep,Agent,Task");
    for name in [".git", ".CLAUDE", ".huſ\u{212a}y", "bracket[dir]"] {
        let path = dir.path().join(name);
        std::fs::create_dir(&path).unwrap();
        config.cwd = path.canonicalize().unwrap();
        let error = config
            .configure(&mut std::process::Command::new("unused"), &launcher)
            .err()
            .unwrap();
        assert_eq!(error.code(), "invalid_path");
    }
    config.cwd = dir.path().canonicalize().unwrap();
    config.sandbox = Sandbox::WorkspaceWrite;
    let mut command = std::process::Command::new("unused");
    let _guard = config.configure(&mut command, &launcher).unwrap();
    let args: Vec<_> = command.get_args().map(|s| s.to_str().unwrap()).collect();
    let settings: Value =
        serde_json::from_str(args[args.iter().position(|a| *a == "--settings").unwrap() + 1])
            .unwrap();
    let deny = settings["sandbox"]["filesystem"]["denyWrite"]
        .as_array()
        .unwrap();
    assert!(deny.contains(&json!(config.cwd.join(".git"))));
    assert!(deny.contains(&json!(launcher.protected_root().unwrap())));
    #[cfg(target_os = "macos")]
    for pattern in [
        ".[hH][uU]ſ\u{212a}[yY]",
        ".[pP][rR][oO]ﬁ[lL][eE]",
        ".[mM][cC][pP].[jJ]ſ[oO][nN]",
    ] {
        assert!(deny.contains(&json!(config.cwd.join(pattern))), "{deny:?}");
    }
}

#[test]
#[cfg(target_os = "linux")] // APFS refuses non-UTF8 names at creation.
fn non_utf8_protected_symlink_targets_fail_without_panicking() {
    use std::os::unix::ffi::OsStringExt;
    let launcher = crate::launch::test_cache().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let target = dir
        .path()
        .join(std::ffi::OsString::from_vec(vec![b'x', 0xff]));
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join(".git")).unwrap();
    let config = SessionConfig {
        sandbox: Sandbox::WorkspaceWrite,
        ..SessionConfig::for_test(
            Uuid::new_v4().to_string(),
            dir.path().canonicalize().unwrap(),
        )
    };
    let error = config
        .configure(&mut std::process::Command::new("unused"), &launcher)
        .err()
        .unwrap();
    assert_eq!(error.code(), "invalid_path");
}

#[test]
fn full_access_native_permission_prompts_allow_external_paths_without_bypassing_shell_receipts() {
    for sandbox in [
        Sandbox::ReadOnly,
        Sandbox::WorkspaceWrite,
        Sandbox::FullAccess,
    ] {
        let mut f =
            Fixture::with_extensions(None, Uuid::new_v4().to_string(), |c| c.sandbox = sandbox);
        f.turn();
        let frame = |tool, input| {
            json!({"type":"control_request","request_id":"prompt","request":{
            "subtype":"can_use_tool","tool_name":tool,"tool_use_id":"tool-1","input":input,
            "decision_reason":"other","blocked_path":"/outside/workspace/file"}})
        };
        let replies = f
            .codec
            .receive(frame(
                "Edit",
                json!({"file_path":"/outside/workspace/file"}),
            ))
            .unwrap();
        assert_eq!(
            replies[0]["response"]["response"]["behavior"],
            if sandbox == Sandbox::FullAccess {
                "allow"
            } else {
                "deny"
            }
        );
    }
    let mut f = Fixture::with_extensions(None, Uuid::new_v4().to_string(), |c| {
        c.sandbox = Sandbox::FullAccess;
    });
    f.turn();
    assert_eq!(
        f.permission(json!({"command":"git commit -am test"})),
        "deny"
    );
    let issued = f.hook("Bash", json!({"command":"git commit -am test"}));
    let response = f.codec.receive(json!({"type":"control_request","request_id":"prompt","request":{
        "subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tool-1","input":issued["updatedInput"],
        "decision_reason":"other","blocked_path":"/outside/workspace/.git"}})).unwrap();
    assert_eq!(response[0]["response"]["response"]["behavior"], "allow");
    assert_eq!(f.permission(issued["updatedInput"].clone()), "deny");
}

#[test]
fn full_access_claude_disables_native_confinement_but_keeps_background_work_denied() {
    let launcher = crate::launch::test_cache().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = SessionConfig {
        sandbox: Sandbox::FullAccess,
        ..SessionConfig::for_test(
            Uuid::new_v4().to_string(),
            dir.path().canonicalize().unwrap(),
        )
    };
    let mut command = Command::new("claude");
    let _guard = config.configure(&mut command, &launcher).unwrap();
    let args: Vec<_> = command.get_args().collect();
    let settings: Value = serde_json::from_str(
        args.windows(2)
            .find(|pair| pair[0] == "--settings")
            .unwrap()[1]
            .to_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(settings["sandbox"]["enabled"], false);
    assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], true);
    assert!(settings["sandbox"].get("filesystem").is_none());
    assert!(settings["sandbox"].get("network").is_none());
    assert_eq!(
        settings["permissions"]["deny"],
        json!([
            "TaskOutput",
            "TaskStop",
            "CronCreate",
            "CronDelete",
            "CronList"
        ])
    );
    assert!(config.writable("/somewhere/outside/workspace/.git/config"));
    assert!(config.writable("relative.txt"));
}

#[test]
fn workspace_write_protects_the_outer_guard_directory_and_allows_only_its_socket() {
    let launcher = crate::launch::test_cache().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = SessionConfig {
        sandbox: Sandbox::WorkspaceWrite,
        ..SessionConfig::for_test(
            Uuid::new_v4().to_string(),
            dir.path().canonicalize().unwrap(),
        )
    };
    let mut command = Command::new("claude");
    let guard = config.configure(&mut command, &launcher).unwrap();
    let args: Vec<_> = command.get_args().collect();
    let settings: Value = serde_json::from_str(
        args.windows(2)
            .find(|pair| pair[0] == "--settings")
            .unwrap()[1]
            .to_str()
            .unwrap(),
    )
    .unwrap();
    let deny = settings["sandbox"]["filesystem"]["denyWrite"]
        .as_array()
        .unwrap();
    assert!(deny.contains(&json!(guard.protected_directory())));
    assert_ne!(guard.directory(), guard.protected_directory());
    assert_eq!(
        settings["sandbox"]["network"]["allowUnixSockets"],
        json!([guard.directory().join("socket")])
    );
}
