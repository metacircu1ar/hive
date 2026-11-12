//! Native fixture setup with isolated provider state and owned HTTP process.
use super::*;

pub(super) fn fixture(case: &'static str) -> NativeFixture {
    let native = PathBuf::from(
        std::env::var_os("HIVE_NATIVE_CLAUDE")
            .expect("set HIVE_NATIVE_CLAUDE to the real supported Claude executable"),
    );
    assert!(native.is_absolute() && native.is_file());
    let f = Harness::builder(Provider::Claude).build();
    let native_version = version(&f, &native);
    let endpoint = Endpoint::start(&f, case);
    let binding = configure(&f, &native, &endpoint, case);
    publish_plugin(&f);
    NativeFixture {
        hive: f,
        _endpoint: endpoint.child,
        trace: endpoint.trace,
        receipts: endpoint.receipts,
        exported: false,
        native_version,
        case,
        usage: Vec::new(),
        histories: Vec::new(),
        provider_home: binding.directory.join("provider"),
        cleanup: None,
    }
}

fn version(f: &Harness, native: &Path) -> String {
    let version_home = f.tmp.path().join("native-version-home");
    fs::create_dir(&version_home).unwrap();
    let version = support::command(native)
        .env_clear()
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
        )
        .env("CLAUDE_CONFIG_DIR", &version_home)
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DISABLE_AUTOUPDATER", "1")
        .arg("--version")
        .output()
        .unwrap();
    assert!(version.status.success());
    let native_version = String::from_utf8(version.stdout).unwrap().trim().to_owned();
    assert!(native_version.len() <= 128 && native_version.contains("Claude Code"));
    native_version
}

struct Endpoint {
    child: support::ChildGuard,
    trace: PathBuf,
    receipts: PathBuf,
    ready: Value,
}
impl Endpoint {
    fn start(f: &Harness, case: &'static str) -> Self {
        let python = f.tmp.path().join("python3");
        let model = f.tmp.path().join("native-model.py");
        fs::write(
            &model,
            include_str!("../../../fixtures/native_claude_model.py"),
        )
        .unwrap();
        let trace = f.tmp.path().join("native-observations.jsonl");
        let receipts = f.tmp.path().join("native-requests.jsonl");
        let ready_file = f.tmp.path().join("native-http-ready");
        let mut endpoint = support::ChildGuard::new(
            support::command(&python)
                .arg(&model)
                .arg("--workspace")
                .arg(f.tmp.path().canonicalize().unwrap())
                .arg("--receipts")
                .arg(&receipts)
                .arg("--identity-binary")
                .arg(std::env::current_exe().unwrap())
                .arg("--ready")
                .arg(&ready_file)
                .arg("--case")
                .arg(case)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        support::wait::wait_until(
            support::wait::STARTUP_TIMEOUT,
            support::wait::POLL_INTERVAL,
            "native fixture HTTP endpoint",
            || {
                assert!(
                    endpoint.try_wait().unwrap().is_none(),
                    "native fixture endpoint exited"
                );
                ready_file.exists().then_some(())
            },
        );
        let ready: Value = serde_json::from_slice(&fs::read(&ready_file).unwrap()).unwrap();
        Self {
            child: endpoint,
            trace,
            receipts,
            ready,
        }
    }
}

fn configure(
    f: &Harness,
    native: &Path,
    endpoint: &Endpoint,
    case: &'static str,
) -> hive::state::Binding {
    let python = f.tmp.path().join("python3");
    let model = f.tmp.path().join("native-model.py");
    fs::write(
        f.tmp.path().join("native-fixture.json"),
        serde_json::to_vec(&json!({
            "binary":native, "url":endpoint.ready["url"], "trace":endpoint.trace,
            "plugin":f.tmp.path().join("native-plugin"),
            "case":case, "workspace":f.tmp.path(),
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        f.tmp.path().join("claude"),
        format!(
            "#!{}\n{}",
            python.display(),
            include_str!("../../../fixtures/native_claude_launcher.py")
        ),
    )
    .unwrap();
    fs::set_permissions(
        f.tmp.path().join("claude"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let key = f
        .tmp
        .path()
        .canonicalize()
        .unwrap()
        .join("native-placeholder-key");
    fs::write(&key, "sk-ant-api03-native-fixture-placeholder").unwrap();
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    let profile = Profile {
        provider: Provider::Claude,
        method: Method::ApiKey,
        api_key_env: None,
        api_key_file: Some(key),
    };
    let binding = State::new(f.root.clone())
        .unwrap()
        .register("native", &profile)
        .unwrap();
    f.edit_config(|config| {
        config["auth_profiles"]["native"] = serde_json::to_value(&profile).unwrap();
        config["roles"]["native"] = json!({"auth":"native", "model":"sonnet", "effort":"low",
                "instructions":"role.md", "sandbox":"workspace-write",
                    "options":{"claude.skills":true}});
        config["mcp_servers"] = json!({"docs":{"command":python, "args":[model,
                "--stdio-docs"]}});
    });
    binding
}

fn publish_plugin(f: &Harness) {
    let plugin = f.tmp.path().join("native-plugin/.claude-plugin");
    fs::create_dir_all(&plugin).unwrap();
    fs::write(
        plugin.join("plugin.json"),
        r#"{"name":"native-fixture","version":"1.0.0","description":"Local protocol fixture"}"#,
    )
    .unwrap();
    let skill = f.tmp.path().join("native-plugin/skills/native-canary");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: native-canary\ndescription: Local native protocol fixture\ncontext: \
            fork\nagent: general-purpose\nbackground: \
            false\n---\nNATIVE_SKILL_CHILD\n",
    )
    .unwrap();
}
