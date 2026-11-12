//! Path ownership regressions.
use super::*;

#[test]
fn private_records_reject_symlinks_and_hardlinks() {
    let (_temp, layout, _, _) = fixture();
    let path = layout.home.join("record");
    files::write(&path, b"ok", false).unwrap();
    let link = layout.home.join("link");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(files::read(&link).is_err());
    fs::remove_file(link).unwrap();
    fs::hard_link(&path, layout.home.join("hard")).unwrap();
    assert!(files::read(&path).is_err());
}

#[test]
fn scope_and_layout_resolve_user_symlinks_but_managed_records_do_not_follow_them() {
    let (_temp, mut layout, _, _) = fixture();
    let real = layout.home.join("real");
    fs::create_dir(&real).unwrap();
    let link = layout.home.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    layout.config = files::location(&link).unwrap();
    assert_eq!(
        files::absolute(&link.join("missing/deeper")).unwrap(),
        real.join("missing/deeper")
    );
    let config = real.join("config.yaml");
    fs::write(&config, "auth_profiles: {}").unwrap();
    let linked_config = layout.home.join("config-link.yaml");
    std::os::unix::fs::symlink(&config, &linked_config).unwrap();
    let scope = Scope::new("hive", &[&linked_config, &link.join("state")]).unwrap();
    assert_eq!(scope.paths[0], config);
    assert_eq!(
        scope,
        Scope::new("hive", &[&config, &real.join("state")]).unwrap()
    );
    let spec = Spec::new(
        &layout,
        scope.clone(),
        crate::service::Program {
            path: std::path::Path::new("/bin/echo"),
            arguments: vec![],
            environment_file: None,
            required_environment: vec![],
        },
    )
    .unwrap();
    let mock = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    ctl.install(&spec).unwrap();
    assert!(ctl.status(&|| Ok(false)).unwrap().managed);
    spec.verify(&layout, &scope).unwrap();
    assert!(spec.descriptor.starts_with(real));
    assert!(files::absolute(&linked_config).is_err());
}

#[test]
fn metadata_observation_distinguishes_missing_from_unavailable_and_dangling() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing");
    assert!(files::optional_metadata(&missing).unwrap().is_none());
    let dangling = temp.path().join("dangling");
    std::os::unix::fs::symlink("missing", &dangling).unwrap();
    assert!(
        files::optional_metadata(&dangling)
            .unwrap()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(files::absolute(&dangling).is_err());
    let looping = temp.path().join("loop");
    std::os::unix::fs::symlink("loop", &looping).unwrap();
    let inaccessible = looping.join("child");
    assert_eq!(
        files::optional_metadata(&inaccessible)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ELOOP)
    );
    assert_eq!(
        files::absolute(&inaccessible).unwrap_err().raw_os_error(),
        Some(libc::ELOOP)
    );
}
