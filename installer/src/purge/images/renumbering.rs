use super::*;
use hive_setup_support::removal::rename_noreplace;

fn renumber(action: Action, field: &str) -> Action {
    let mut value = serde_json::to_value(action).unwrap();
    value["object"]["identity"][field] =
        serde_json::json!(value["object"]["identity"][field].as_u64().unwrap() ^ 1);
    serde_json::from_value(value).unwrap()
}

#[test]
fn renumbered_image_quarantine_retry_refuses_changed_inode() {
    for field in ["device", "inode"] {
        let temporary = tempfile::tempdir().unwrap();
        let manager = temporary.path().join("manager");
        fs::create_dir(&manager).unwrap();
        fs::write(manager.join("remaining"), b"partial removal").unwrap();
        let action = Action::capture(&manager, "images").unwrap();
        fs::rename(&manager, &action.quarantine).unwrap();
        let mut action = renumber(action, field);
        let saved = action.object.identity.clone();
        let result = remove_with_scan(&mut action, |_| Ok(()), rename_noreplace);
        assert_eq!(result.is_ok(), field == "device");
        assert_eq!(action.object.identity, saved);
        assert_eq!(action.quarantine.exists(), field != "device");
        assert!(!manager.exists());
    }
}

fn fixture() -> (tempfile::TempDir, Action) {
    let temporary = tempfile::tempdir().unwrap();
    let prefix = temporary.path().join("prefix");
    let part = crate::tests::fixture(temporary.path(), "hive");
    publish::install(&prefix, temporary.path(), &[part], |_| Ok(())).unwrap();
    crate::uninstall::remove_with(
        &prefix,
        &["hive".into()],
        false,
        &mut crate::lifecycle::Inert,
        |_| Ok(()),
    )
    .unwrap();
    let action = Action::capture(&prefix.join("lib/hive"), "images").unwrap();
    (temporary, action)
}

#[test]
fn renumbered_image_scan_failure_restores_only_the_exact_live_rename() {
    for defect in ["none", "saved_inode", "quarantine_inode"] {
        let (_temporary, action) = fixture();
        let mut action = renumber(action, "device");
        if defect == "saved_inode" {
            action = renumber(action, "inode");
        }
        let original = action.object.path.clone();
        let quarantine = action.quarantine.clone();
        let retained = quarantine.with_extension("retained");
        let live = Identity::read(&original).unwrap();
        let mut scans = 0;
        let result = remove_with_scan(
            &mut action,
            |_| {
                scans += 1;
                if scans == 1 {
                    return Ok(());
                }
                if defect == "quarantine_inode" {
                    fs::rename(&quarantine, &retained)?;
                    fs::create_dir(&quarantine)?;
                }
                Err(std::io::Error::from_raw_os_error(libc::EBUSY))
            },
            rename_noreplace,
        );
        assert!(result.is_err());
        assert!(!action.done);
        if defect == "quarantine_inode" {
            assert!(!original.exists());
            assert_eq!(Identity::read(&retained).unwrap(), live);
            assert_ne!(Identity::read(&quarantine).unwrap(), live);
        } else {
            assert_eq!(Identity::read(&original).unwrap(), live);
            assert!(!quarantine.exists());
            validate(&original).unwrap();
        }
        assert_eq!(scans, if defect == "saved_inode" { 0 } else { 2 });
    }
}
