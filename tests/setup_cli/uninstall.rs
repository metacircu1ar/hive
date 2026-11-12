// Uninstall input/answer scenarios retain their parent test namespace.
#[test]
fn uninstall_scope_requires_purge_before_executing_helper() {
    let f = Fixture::new();
    let prefix = f.home.path().join("installation");
    hive_setup_support::files::directory(&prefix.join("bin")).unwrap();
    atomic_fs::Publisher::new(
        (hive_setup_support::names::PUBLICATION_OPTIONS)
            .with_mode(0o700)
            .with_durability(atomic_fs::Durability::AtomicVisible),
    )
    .create_new(prefix.join("bin/hive-install"), b"#!/bin/sh\nexit 91\n")
    .unwrap();
    assert_eq!(f.removal().output().unwrap().status.code(), Some(91));
    for flag in [
        "--comb-root",
        "--bridge-file",
        "--config",
        "--state-dir",
        "--purge-comb-root",
    ] {
        let (out, value) = output(f.removal().arg(flag).arg(f.home.path().join("scope")));
        let value = support::reply::Failure::parse(value);
        assert_eq!(out.status.code(), Some(2), "{value:?}");
        assert_eq!(value.error.code, "conflicting_arguments");
        assert_eq!(value.error.field.as_deref().unwrap(), flag);
        assert!(value.error.expected.as_deref().unwrap().contains("--purge"));
        assert!(String::from_utf8_lossy(&out.stderr).contains("Examples:"));
        let allowed = f
            .removal()
            .arg("--purge")
            .arg(flag)
            .arg(f.home.path().join("scope"))
            .output()
            .unwrap();
        assert_eq!(allowed.status.code(), Some(91));
    }
    assert!(!f.config.exists());
    assert!(!f.state.exists());
}

#[test]
fn uninstall_rejects_supplied_wizard_values_and_globals_before_helper() {
    let f = Fixture::new();
    for (flag, value) in [
        ("--provider", "codex"),
        ("--default-provider", "claude"),
        ("--login", "skip"),
        ("--codex-profile", "codex-main"),
        ("--claude-profile", "claude-main"),
        ("--codex-model", "model"),
        ("--claude-model", "sonnet"),
        ("--codex-effort", "low"),
        ("--claude-effort", "high"),
        ("--embedding-config", "missing"),
        ("--comb-bin", "missing"),
        ("--no-comb", "false"),
        ("--no-comb-roles", "false"),
        ("--collectors", "false"),
        ("--user-skills", "codex"),
        ("--skills-dir", "missing"),
        ("--install-provider", "claude"),
        ("--services", "false"),
        ("--no-services", "false"),
        ("--restart-daemon", "false"),
        ("--preserve-stopped-services", "false"),
        ("--service-environment", "missing"),
        ("--service-path", "/bin"),
        ("--codex-bin", "codex"),
        ("--claude-bin", "claude"),
        ("--probe-timeout", "15"),
        ("--login-timeout", "600"),
    ] {
        let (out, value) = output(f.removal().arg(format!("{flag}={value}")));
        let value = support::reply::Failure::parse(value);
        assert_eq!(out.status.code(), Some(2), "{flag}: {value:?}");
        assert_eq!(value.error.code, "conflicting_arguments");
        assert_eq!(value.error.field.as_deref().unwrap(), flag);
    }
    for flag in ["--config", "--state-dir"] {
        let (out, value) = output(f.removal().arg(flag).arg("missing"));
        let value = support::reply::Failure::parse(value);
        assert_eq!(out.status.code(), Some(2), "{value:?}");
        assert_eq!(value.error.field.as_deref().unwrap(), flag);
        assert!(value.error.expected.as_deref().unwrap().contains("--purge"));
    }
    assert!(!f.home.path().join("installation").exists());
    assert!(!f.config.exists());
    assert!(!f.state.exists());
}

#[test]
fn uninstall_answers_preserve_false_presence_and_supplied_service_flag() {
    let f = Fixture::new();
    let answers = f.home.path().join("removal.yaml");
    for flag in [
        "services",
        "no-services",
        "restart-daemon",
        "preserve-stopped-services",
    ] {
        hive_setup_support::files::write(
            &answers,
            format!("uninstall: true\n{flag}: false\n").as_bytes(),
            true,
        )
        .unwrap();
        let mut command = support::command(env!("CARGO_BIN_EXE_hivectl"));
        f.environment(&mut command);
        let (out, value) = output(command.args(["setup", "--answers"]).arg(&answers));
        let value = support::reply::Failure::parse(value);
        assert_eq!(out.status.code(), Some(2), "{value:?}");
        assert_eq!(value.error.field.as_deref().unwrap(), "--answers");
        assert!(value.error.expected.as_deref().unwrap().contains(flag));
    }
    hive_setup_support::files::write(&answers, b"uninstall: true\n", true).unwrap();
    let mut command = support::command(env!("CARGO_BIN_EXE_hivectl"));
    f.environment(&mut command);
    let (out, value) = output(
        command
            .args(["--probe-timeout", "15", "setup", "--answers"])
            .arg(&answers),
    );
    let value = support::reply::Failure::parse(value);
    assert_eq!(out.status.code(), Some(2), "{value:?}");
    assert_eq!(value.error.field.as_deref().unwrap(), "--probe-timeout");
}
