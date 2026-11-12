//! Catalog discovery regressions.
use super::*;

#[test]
fn standalone_catalog_discovery_does_not_require_a_daemon_lock() {
    let f = Harness::new();
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--config")
        .arg(&f.config)
        .arg("--state-dir")
        .arg(&f.root)
        .arg("--codex-bin")
        .arg(&f.binary)
        .args(["models", "--auth", "main"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        result["models"]["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model["model"] == "gpt-5.6-luna")
    );
}

#[test]
fn cold_model_discovery_uses_the_agents_pinned_generation_after_repair() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({"catalog_models":["gpt-5.6-terra","old-catalog-only"]}))
        .build();
    f.start();
    let id = f.spawn("hold");
    let original = f.wait(&id, |a| a.state == "running");
    f.shutdown();
    let state = State::new(f.root.clone()).unwrap();
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let repair = state.begin_repair("main", &profile).unwrap();
    state.finish_repair(&repair, true, None).unwrap();
    assert_ne!(
        state
            .lookup("main", &profile)
            .unwrap()
            .unwrap()
            .id
            .to_string(),
        original.auth_generation
    );
    f.start();
    let changed = change(&f, &id, "old-catalog-only", "medium");
    assert_eq!(changed["desired"]["model"], "old-catalog-only");
    assert_eq!(f.status(&id).auth_generation, original.auth_generation);
    f.shutdown();
}

#[test]
fn cold_catalog_lookup_returns_busy_when_the_only_provider_slot_is_starting() {
    let mut f = Harness::new();
    f.edit_config(|config| {
        config["runtime"] =
            json!({"max_providers":1,"max_loaded":1,"max_starting":1,"max_active_turns":1});
    });
    f.start();
    let id = f.spawn("hold");
    f.wait(&id, |a| a.state == "running");
    f.shutdown();
    let pid = f.tmp.path().join("provider-started");
    f.set_scenario(
        Provider::Codex,
        json!({"model_list_delay":2,"pid_file":pid}),
    );
    f.start();
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    wait_until(
        STARTUP_TIMEOUT,
        PROVIDER_POLL_INTERVAL,
        "provider startup",
        || pid.exists().then_some(()),
    );
    let response = f.expect_err(
        json!({"op":"set_options","target":id,"options":{"model":"gpt-5.6-luna"}}),
        "agent_busy",
    );
    assert_eq!(response.error.code, "agent_busy", "{response:?}");
    f.wait(&id, |a| a.state == "running");
    // A catalog cache hit needs no extra process, even with that slot occupied.
    change(&f, &id, "gpt-5.6-luna", "medium");
    f.shutdown();
}

#[test]
fn claude_catalog_waits_for_clean_exit_and_rejects_failed_or_forced_exit() {
    for (delay, exit, expected_error) in [
        (3, 0, None),
        (0, 7, Some("provider_failed")),
        (30, 0, Some("provider_shutdown_forced")),
    ] {
        let f = super::super::claude::fixture();
        let executable = f.tmp.path().join("claude-discovery");
        let pid_file = f.tmp.path().join("discovery-pid");
        fs::write(
            &executable,
            format!(
                r#"#!{}
import json, os, pathlib, sys, time
if "--version" in sys.argv:
    print("2.1.285 (Claude Code)")
    sys.exit(0)
pathlib.Path({:?}).write_text(str(os.getpid()))
request = json.loads(sys.stdin.readline())
assert request['request']['subtype'] == 'initialize'
print(json.dumps({{'type':'control_response','response':{{'subtype':'success',
 'request_id':request['request_id'],'response':{{
 'account':{{'apiProvider':'firstParty','subscriptionType':'Team'}},
 'models':[{{'value':'sonnet','resolvedModel':'claude-sonnet-test','displayName':'Sonnet',
 'description':'mock','supportedEffortLevels':['low']}}]}}}}}}), flush=True)
assert not sys.stdin.read(), 'discovery must never submit input'
time.sleep({delay})
sys.exit({exit})
"#,
                f.tmp.path().join("python3").display(),
                pid_file.to_str().unwrap()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--config")
            .arg(&f.config)
            .arg("--state-dir")
            .arg(&f.root)
            .arg("--claude-bin")
            .arg(&executable)
            .args(["models", "--auth", "claude"])
            .output()
            .unwrap();
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        if let Some(error) = expected_error {
            assert!(!output.status.success());
            let result = support::reply::Failure::parse(result);
            assert_eq!(result.error.code, error, "{result:?}");
        } else {
            assert!(output.status.success(), "{result:?}");
            assert_eq!(result["models"]["data"][0]["model"], "sonnet");
        }
        let pid = fs::read_to_string(pid_file).unwrap();
        let probe = support::command("ps")
            .args(["-p", pid.trim(), "-o", "pid="])
            .output()
            .unwrap();
        assert!(
            probe.stdout.is_empty(),
            "discovery helper must be gone before returning"
        );
    }
}
