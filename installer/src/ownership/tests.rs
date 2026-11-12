use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn fresh_install_upgrade_and_uninstall_work_under_isolated_umask_two() {
    let executable = std::env::current_exe().unwrap();
    let output = hive_setup_support::runner::capture(
        std::process::Command::spawn,
        executable.to_str().unwrap(),
        &[
            "--exact".into(),
            "ownership::tests::umask_two_fixture".into(),
            "--ignored".into(),
        ],
        std::time::Duration::from_secs(30),
    )
    .unwrap();
    assert_eq!(output.code, 0, "{}", output.stdout);
    assert!(output.stdout.contains("1 passed"));
}
#[test]
#[ignore = "owned subprocess only: umask changes must not affect shared test threads"]
fn umask_two_fixture() {
    let temp = tempfile::tempdir().unwrap();
    let prefix = temp.path().join("prefix");
    // Prepare source fixtures before changing umask; production archive staging
    // has explicit private modes and is covered separately.
    let part = crate::tests::fixture(temp.path(), "hive");
    let _previous_umask = rustix::process::umask(rustix::fs::Mode::from_bits_retain(0o002));
    for _ in 0..2 {
        crate::publish::install(
            &prefix,
            temp.path(),
            std::slice::from_ref(&part),
            |_| Ok(()),
        )
        .unwrap();
    }
    let manager = prefix.join("lib/hive");
    assert_eq!(fs::metadata(&manager).unwrap().mode() & 0o777, 0o700);
    assert_eq!(
        fs::metadata(manager.join("generations")).unwrap().mode() & 0o022,
        0
    );
    crate::uninstall::remove_with(
        &prefix,
        &["hive".into()],
        false,
        &mut crate::lifecycle::Inert,
        |_| Ok(()),
    )
    .unwrap();
}

#[test]
fn user_group_writable_routes_and_inner_directories_survive_upgrade_and_uninstall() {
    if rustix::process::geteuid().as_raw() == 0 {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let prefix = temp.path().join("prefix");
    fs::create_dir(&prefix).unwrap();
    fs::create_dir(prefix.join("lib")).unwrap();
    fs::set_permissions(prefix.join("lib"), fs::Permissions::from_mode(0o775)).unwrap();
    let part = crate::tests::fixture(temp.path(), "hive");
    crate::publish::install(
        &prefix,
        temp.path(),
        std::slice::from_ref(&part),
        |_| Ok(()),
    )
    .unwrap();
    let manager = prefix.join("lib/hive");
    let image = manager.join(crate::publish::current(&manager).unwrap().unwrap());
    for path in [
        manager.join("generations"),
        image.clone(),
        image.join("bin"),
        image.join("share"),
    ] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o775)).unwrap();
    }
    crate::publish::install(&prefix, temp.path(), &[part], |_| Ok(())).unwrap();
    crate::uninstall::remove_with(
        &prefix,
        &["hive".into()],
        false,
        &mut crate::lifecycle::Inert,
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(
        fs::metadata(prefix.join("lib")).unwrap().mode() & 0o777,
        0o775
    );
    assert_eq!(fs::metadata(manager).unwrap().mode() & 0o777, 0o700);
}

#[test]
fn world_writable_routes_refuse_before_source_execution_or_prefix_publication() {
    let temp = tempfile::tempdir().unwrap();
    let route = temp.path().join("route");
    fs::create_dir(&route).unwrap();
    fs::set_permissions(&route, fs::Permissions::from_mode(0o777)).unwrap();
    let prefix = route.join("prefix");
    let part = crate::tests::fixture(temp.path(), "hive");
    let error = crate::publish::install(&prefix, temp.path(), &[part], |_| Ok(()))
        .err()
        .unwrap();
    assert!(error.to_string().contains("untrusted directory"));
    assert!(!prefix.exists());
    fs::set_permissions(route, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn forged_public_manager_is_never_adopted_even_with_exact_owner_contents() {
    let temp = tempfile::tempdir().unwrap();
    let prefix = temp.path().join("prefix");
    let manager = prefix.join("lib/hive");
    fs::create_dir_all(manager.join("generations")).unwrap();
    fs::set_permissions(&manager, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(manager.join("owner"), crate::publish::OWNER).unwrap();
    let part = crate::tests::fixture(temp.path(), "hive");
    let error = crate::publish::install(&prefix, temp.path(), &[part], |_| Ok(()))
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("ownership or permissions are unsafe")
    );
    assert_eq!(
        fs::read(manager.join("owner")).unwrap(),
        crate::publish::OWNER
    );
    assert!(!prefix.join("lib/.hive-install.lock").exists());
    assert!(!manager.join("current").exists());
}

#[test]
fn foreign_owned_private_manager_is_refused_before_lock_or_payload_access() {
    if rustix::process::geteuid().as_raw() != 0 {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let prefix = temp.path().join("prefix");
    let manager = prefix.join("lib/hive");
    fs::create_dir_all(manager.join("generations")).unwrap();
    fs::set_permissions(&manager, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(manager.join("owner"), crate::publish::OWNER).unwrap();
    rustix::fs::chownat(
        rustix::fs::CWD,
        &manager,
        Some(rustix::process::Uid::from_raw(65534)),
        None,
        rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
    )
    .unwrap();
    let part = crate::tests::fixture(temp.path(), "hive");
    assert!(crate::publish::install(&prefix, temp.path(), &[part], |_| Ok(())).is_err());
    assert!(!prefix.join("lib/.hive-install.lock").exists());
    assert_eq!(
        fs::read(manager.join("owner")).unwrap(),
        crate::publish::OWNER
    );
}
