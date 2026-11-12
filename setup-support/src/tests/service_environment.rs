//! Service environment regressions.
use super::*;

#[test]
fn environment_is_private_literal_and_not_copied_into_units() {
    let (_temp, layout, scope, mut spec) = fixture();
    let path = layout.home.join("secret.env");
    files::write(&path, b"API_KEY=literal $(do-not-run) $HOME\n", false).unwrap();
    assert_eq!(
        environment::read(Some(&path)).unwrap()["API_KEY"],
        "literal $(do-not-run) $HOME"
    );
    spec.environment_file = Some(path.clone());
    spec.required_environment = vec!["API_KEY".into()];
    spec.check_environment().unwrap();
    assert!(
        !String::from_utf8(spec.render().unwrap())
            .unwrap()
            .contains("do-not-run")
    );
    for bad in [
        "A=1\nA=2",
        "bad-key=secret",
        "X=bad\0value",
        "HOME=foreign",
        "export A=1",
    ] {
        files::write(&path, bad.as_bytes(), true).unwrap();
        assert!(environment::read(Some(&path)).is_err());
    }
    files::write(&path, b"OTHER=value", true).unwrap();
    let mock = Mock::new();
    assert!(
        Controller {
            layout: &layout,
            scope: &scope,
            runner: &mock
        }
        .install(&spec)
        .is_err()
    );
    assert!(!spec.unit.exists());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(environment::read(Some(&path)).is_err());
}

#[test]
fn explicit_service_path_rejects_invalid_entries_and_deduplicates_toolchains() {
    let (_temp, layout, _, mut spec) = fixture();
    let path = format!(
        "{}:/usr/bin:{}",
        layout.home.display(),
        layout.home.display()
    );
    let captured = environment::service_path(Some(&path)).unwrap();
    for invalid in [
        "",
        ".",
        "relative",
        "/missing-toolchain",
        "/usr/bin:",
        "/usr/bin:.",
        "/usr/bin:/missing-toolchain",
        "/usr/bin:\n",
    ] {
        assert!(
            environment::service_path(Some(invalid)).is_err(),
            "{invalid:?}"
        );
    }
    assert_eq!(captured, format!("{}:/usr/bin", layout.home.display()));
    spec.environment.insert("PATH".into(), captured.clone());
    assert!(
        String::from_utf8(spec.render().unwrap())
            .unwrap()
            .contains(&format!("<key>PATH</key><string>{captured}</string>"))
    );
    let program = layout.home.join("provider");
    fs::write(&program, "#!/usr/bin/env custom-interpreter\n").unwrap();
    assert!(
        environment::interpreter_warning(&program, &captured)
            .unwrap()
            .is_some()
    );
    let interpreter = layout.home.join("custom-interpreter");
    fs::write(&interpreter, "#!/bin/sh\n").unwrap();
    fs::set_permissions(interpreter, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        environment::interpreter_warning(&program, &captured)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        runner::capture(
            std::process::Command::spawn,
            "/bin/sh",
            &["-c".into(), "echo cause >&2; exit 5".into()],
            Duration::from_secs(1)
        )
        .unwrap()
        .stderr
        .trim(),
        "cause"
    );
}
