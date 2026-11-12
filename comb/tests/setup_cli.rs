#![expect(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact reviewed maintainer write inventory"
)]
//! Setup default paths and stored socket routing through actual release binaries.
#![expect(
    clippy::unwrap_used,
    reason = "Fixture setup and assertions must fail on unexpected errors."
)] // Test harness setup and decoded fixture assertions.
#[path = "../../tests/support/reply.rs"]
#[expect(
    clippy::expect_used,
    reason = "The shared test observer must fail on malformed fixture replies."
)]
mod reply;

use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let cleanup = (|| -> std::io::Result<()> {
            if self.0.try_wait()?.is_none() {
                self.0.kill()?;
            }
            self.0.wait()?;
            Ok(())
        })();
        if std::thread::panicking() {
            if let Err(error) = cleanup {
                eprintln!("fixture process cleanup also failed: {error}");
            }
        } else {
            assert!(
                cleanup.is_ok(),
                "fixture process cleanup failed: {cleanup:?}"
            );
        }
    }
}
fn command(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_comb"));
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .current_dir(home);
    cmd
}
fn json(command: &mut Command) -> Value {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn prepared_setup_plans_without_initializing_and_applies_bound_options() {
    use hive_setup_support::preparation::{self, Resources};
    let temp = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let home = temp.path().canonicalize().unwrap();
    let root = home.join("knowledge");
    let prefix = home.join("installation");
    let resources = home.join("resources.json");
    let answers = home.join("answers.json");
    preparation::write(
        &resources,
        &Resources {
            format: hive_input::compatibility::Exact,
            prefix: prefix.clone(),
            hive_source: None,
            comb_source: Some(home.clone()),
            llama_source: None,
        },
    )
    .unwrap();
    let plan = json(
        command(&home)
            .arg("--root")
            .arg(&root)
            .args(["setup", "--no-services", "--prepare-resources"])
            .arg(&resources)
            .arg("--prepare-output")
            .arg(&answers),
    );
    assert_eq!(plan["dry_run"], true);
    assert!(!root.exists());
    assert!(!prefix.exists());
    let checksum = preparation::digest(&std::fs::read(&answers).unwrap());
    std::fs::create_dir_all(prefix.join("bin")).unwrap();
    let executable = prefix.join("bin/comb");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_comb"), &executable).unwrap();
    let original = command(&home);
    let result = json(
        Command::new(&executable)
            .envs(
                original
                    .get_envs()
                    .filter_map(|(key, value)| value.map(|value| (key, value))),
            )
            .current_dir(&home)
            .arg("--root")
            .arg(&root)
            .args(["setup", "--apply-prepared"])
            .arg(&answers)
            .arg("--prepared-sha256")
            .arg(checksum),
    );
    assert_eq!(result["dry_run"], false);
    assert_eq!(result["changed"], true);
    assert!(root.join(".comb/serve.json").exists());
}

#[test]
fn local_preparation_checks_the_staged_server_before_saving_or_initializing() {
    use hive_setup_support::{
        preparation::{self, Resources},
        runner,
    };
    for staged in [false, true] {
        let temp = tempfile::Builder::new()
            .prefix("comb-preflight-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        let home = temp.path().canonicalize().unwrap();
        let root = home.join("root");
        let source = home.join("comb-llama");
        let marker = home.join("version-ran");
        if staged {
            std::fs::create_dir_all(source.join("bin")).unwrap();
            let binary = source.join("bin/comb-llama-server");
            std::fs::write(
                &binary,
                "#!/bin/sh\nprintf ran > \"$HIVE_LLAMATEST_VERSION_MARKER\"\nexit 4\n",
            )
            .unwrap();
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let resources = home.join("resources.json");
        let answers = home.join("answers.json");
        preparation::write(
            &resources,
            &Resources {
                format: hive_input::compatibility::Exact,
                prefix: home.join("installation"),
                hive_source: None,
                comb_source: Some(home.clone()),
                llama_source: staged.then_some(source),
            },
        )
        .unwrap();
        let mut cmd = command(&home);
        cmd.env("HIVE_LLAMATEST_VERSION_MARKER", &marker)
            .env("XDG_CACHE_HOME", home.join("cache"))
            .arg("--root")
            .arg(&root)
            .args([
                "setup",
                "--yes",
                "--local-embeddings",
                "--no-services",
                "--prepare-resources",
            ])
            .arg(&resources)
            .arg("--prepare-output")
            .arg(&answers);
        let output =
            runner::capture_command(Command::spawn, &mut cmd, Duration::from_secs(10), &|| false)
                .unwrap();
        assert_ne!(output.code, 0);
        let value: Value = serde_json::from_str(&output.stdout).unwrap();
        let expected = if staged {
            comb::Error::llama_server_version_failed()
        } else {
            comb::Error::llama_server_missing()
        };
        assert_eq!(value["error"]["message"], expected.message.as_ref());
        assert_eq!(marker.exists(), staged);
        assert!(!root.exists());
        assert!(!answers.exists());
        assert!(!home.join("installation").exists());
        assert!(!home.join("cache").exists());
    }
}

#[test]
fn installer_preparation_rejects_single_consumer_embedding_input_before_reading() {
    let temp = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let home = temp.path().canonicalize().unwrap();
    let fifo = home.join("embedding.fifo");
    let native = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: private live NUL-terminated pathname and owner-only mode.
    assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
    let output = command(&home)
        .arg("--root")
        .arg(home.join("knowledge"))
        .args(["setup", "--no-services", "--prepare-resources"])
        .arg(home.join("resources"))
        .arg("--prepare-output")
        .arg(home.join("answers"))
        .arg("--embedding-config")
        .arg(&fifo)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["error"]["field"], "--embedding-config");
    assert!(!home.join("knowledge").exists());
    assert!(!home.join("answers").exists());
}
#[test]
fn setup_defaults_to_visible_root_and_commands_inherit_its_custom_socket() {
    let temp = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let home = temp.path().canonicalize().unwrap();
    let socket = home.join("custom.sock");
    let preview = json(command(&home).arg("--socket").arg(&socket).args([
        "setup",
        "--no-services",
        "--for-hive",
        "--dry-run",
        "--yes",
    ]));
    assert_eq!(preview["plan"]["root"], home.join("Comb").to_str().unwrap());
    assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0);
    let initial = json(command(&home).arg("--socket").arg(&socket).args([
        "setup",
        "--no-services",
        "--for-hive",
        "--yes",
    ]));
    assert_eq!(initial["changed"], true);
    let again = json(command(&home).args(["setup", "--no-services", "--for-hive", "--yes"]));
    assert_eq!(again["changed"], false);
    let root = home.join("Comb");
    let mut process = Running(
        command(&home)
            .arg("--root")
            .arg(&root)
            .arg("serve")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::os::unix::net::UnixStream::connect(&socket).is_err() {
        assert!(process.0.try_wait().unwrap().is_none());
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let stats = json(command(&home).arg("--root").arg(&root).arg("stats"));
    assert_eq!(stats["pages"], 0);
}

#[path = "support/embedding_http.rs"]
#[expect(
    clippy::unwrap_used,
    reason = "The fixture HTTP server must fail on unexpected setup and transport errors."
)]
mod embedding_http;
#[test]
fn http_setup_selects_exact_configuration_and_reuses_it_without_endpoint() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let home = temp.path().canonicalize().unwrap();
    let server = embedding_http::Server::new();
    let config = home.join("embedding.json");
    std::fs::write(
        &config,
        serde_json::json!({"backend":"http","model":"fixture",
        "revision":"exact","dimensions":2,"url":server.url,
            "timeout_seconds":1})
        .to_string(),
    )
    .unwrap();
    let initial = json(
        command(&home)
            .args(["setup", "--no-services", "--embedding-config"])
            .arg(&config),
    );
    assert_eq!(initial["changed"], true);
    assert_eq!(initial["plan"]["http"]["model"], "fixture");
    let root = home.join("Comb");
    let saved = std::fs::read(root.join(".comb/serve.json")).unwrap();
    drop(server);
    let again = json(
        command(&home)
            .args(["setup", "--no-services", "--embedding-config"])
            .arg(&config),
    );
    assert_eq!(again["changed"], false);
    assert_eq!(std::fs::read(root.join(".comb/serve.json")).unwrap(), saved);
    // A different unreachable candidate must retain the previous selected configuration.
    let mut candidate: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
    candidate["revision"] = "different".into();
    std::fs::write(&config, candidate.to_string()).unwrap();
    let failed = command(&home)
        .args(["setup", "--no-services", "--embedding-config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert_eq!(std::fs::read(root.join(".comb/serve.json")).unwrap(), saved);
}

#[test]
fn setup_pipe_configuration_has_one_consumer_through_apply() {
    use std::io::{Read as _, Write as _};
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let server = embedding_http::Server::new();
    let bytes = serde_json::json!({"backend":"http","model":"pipe-fixture","revision":"once",
        "dimensions":2,"url":server.url,"timeout_seconds":1})
    .to_string();
    let mut child = Running(
        command(dir.path())
            .args(["setup", "--no-services", "--embedding-config", "/dev/stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    child
        .0
        .stdin
        .take()
        .unwrap()
        .write_all(bytes.as_bytes())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "setup reread or blocked on consumed pipe"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(status.success(), "{stdout}\n{stderr}");
    let result: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(result["plan"]["http"]["revision"], "once");
    assert_eq!(result["changed"], true);
    let saved: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("Comb/.comb/serve.json")).unwrap())
            .unwrap();
    assert_eq!(saved["embeddings"]["model"], "pipe-fixture");
}

#[test]
fn malformed_setup_config_is_reported_before_home_and_managed_state() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("input");
    std::fs::write(&config, b"PRIVATE: [").unwrap();
    let output = command(dir.path())
        .env_remove("HOME")
        .env("XDG_CONFIG_HOME", "invalid-relative")
        .args(["setup", "--embedding-config"])
        .arg(&config)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    let reply = crate::reply::Failure::parse(reply);
    assert_eq!(reply.error.field.as_deref(), Some("--embedding-config"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE"));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}
