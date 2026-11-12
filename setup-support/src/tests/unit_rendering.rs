//! Unit rendering regressions.
use super::*;

#[test]
fn renderers_escape_literals_and_keep_stable_program_symlink() {
    let (_temp, layout, scope, spec) = fixture();
    let xml = String::from_utf8(spec.render().unwrap()).unwrap();
    assert!(xml.contains("&amp; &lt;tag&gt;"));
    assert!(xml.contains("SuccessfulExit</key><false/>"));
    let mut systemd = spec.clone();
    systemd.platform = Platform::Systemd;
    let unit = String::from_utf8(systemd.render().unwrap()).unwrap();
    assert!(unit.contains("$$variable %%specifier"));
    assert!(unit.contains("Restart=on-failure"));
    assert!(!unit.contains("--replace-embeddings"));
    systemd.arguments.push("line\nbreak".into());
    assert!(systemd.render().is_err());
    let stable = layout.home.join("comb");
    std::os::unix::fs::symlink("/bin/echo", &stable).unwrap();
    let spec = Spec::new(
        &layout,
        scope,
        crate::service::Program {
            path: &stable,
            arguments: vec!["serve".into()],
            environment_file: None,
            required_environment: vec![],
        },
    )
    .unwrap();
    assert_eq!(spec.program, stable);
}

#[test]
fn renderer_changes_do_not_confuse_owned_units_with_user_edits() {
    let (_temp, layout, scope, spec) = fixture();
    let runner = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    };
    ctl.install(&spec).unwrap();
    let mut old = spec.clone();
    old.unit_contents.push_str("\n<!-- previous renderer -->\n");
    files::write(&old.unit, old.unit_contents.as_bytes(), true).unwrap();
    files::write(&old.descriptor, &files::encode(&old).unwrap(), true).unwrap();
    assert!(ctl.status(&|| Ok(false)).unwrap().managed);
    ctl.start(&|| Ok(runner.ready.get())).unwrap();
    ctl.stop(&|| Ok(runner.ready.get())).unwrap();
    ctl.install(&spec).unwrap();
    assert_eq!(
        files::read(&spec.unit).unwrap().unwrap(),
        spec.unit_contents.as_bytes()
    );
    files::write(&old.unit, old.unit_contents.as_bytes(), true).unwrap();
    files::write(&old.descriptor, &files::encode(&old).unwrap(), true).unwrap();
    ctl.uninstall(&|| Ok(false)).unwrap();
    ctl.install(&spec).unwrap();
    files::write(&spec.unit, b"user edit", true).unwrap();
    assert!(ctl.install(&spec).is_err());
    assert!(ctl.uninstall(&|| Ok(false)).is_err());
}
