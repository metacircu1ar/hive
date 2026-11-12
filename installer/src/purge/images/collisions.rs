use super::*;
use hive_setup_support::removal::rename_noreplace;
use rustix::io::Errno;

fn fixture() -> (tempfile::TempDir, Action) {
    let temp = tempfile::tempdir().unwrap();
    let prefix = temp.path().join("prefix");
    let part = crate::tests::fixture(temp.path(), "hive");
    publish::install(&prefix, temp.path(), &[part], |_| Ok(())).unwrap();
    crate::uninstall::remove_with(
        &prefix,
        &["hive".into()],
        false,
        &mut crate::lifecycle::Inert,
        |_| Ok(()),
    )
    .unwrap();
    let action = Action::capture(&prefix.join("lib/hive"), "collision").unwrap();
    (temp, action)
}

#[test]
fn unknown_generation_and_pointer_preparations_refuse_purge_unchanged() {
    for pointer in [false, true] {
        let (_temp, mut action) = fixture();
        let manager = &action.object.path;
        let preparation = if pointer {
            manager.join(".pointer-unknown")
        } else {
            manager.join("generations/.preparing-unknown")
        };
        fs::create_dir(&preparation).unwrap();
        fs::write(preparation.join("evidence"), b"retain").unwrap();
        let identity = Identity::read(&preparation).unwrap();
        let error = remove(&mut action).unwrap_err();
        assert!(error.to_string().contains("image purge refused"));
        assert!(error.to_string().contains("recovery evidence"));
        assert_eq!(Identity::read(&preparation).unwrap(), identity);
        assert_eq!(fs::read(preparation.join("evidence")).unwrap(), b"retain");
        assert!(!action.done);
        assert!(action.object.path.is_dir());
    }
}

#[test]
fn image_quarantine_and_rollback_preserve_foreign_empty_directory_identity() {
    for rollback in [false, true] {
        let (_temp, mut action) = fixture();
        let admitted = action.object.identity.clone();
        let original = action.object.path.clone();
        let quarantine = action.quarantine.clone();
        let mut scans = 0;
        let mut renames = 0;
        let mut foreign = None;
        let error = remove_with_scan(
            &mut action,
            |_| {
                scans += 1;
                if scans == 2 {
                    Err(std::io::Error::from_raw_os_error(
                        Errno::BUSY.raw_os_error(),
                    ))
                } else {
                    Ok(())
                }
            },
            |source, destination| {
                renames += 1;
                if renames == if rollback { 2 } else { 1 } {
                    fs::create_dir(destination)?;
                    foreign = Identity::read(destination)?;
                }
                rename_noreplace(source, destination)
            },
        )
        .unwrap_err();
        assert!(!action.done);
        assert_eq!(renames, if rollback { 2 } else { 1 });
        let (owned_path, foreign_path) = if rollback {
            (&quarantine, &original)
        } else {
            (&original, &quarantine)
        };
        assert_eq!(Identity::read(owned_path).unwrap(), admitted);
        assert_eq!(Identity::read(foreign_path).unwrap(), foreign);
        assert_eq!(fs::read_dir(foreign_path).unwrap().count(), 0);
        if !rollback {
            let errno = error
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error()
                .unwrap();
            assert!([Errno::EXIST.raw_os_error(), Errno::NOTEMPTY.raw_os_error()].contains(&errno));
        }
    }
}

#[test]
fn unsupported_image_rename_never_falls_back_or_removes_evidence() {
    for rollback in [false, true] {
        for errno in [Errno::INVAL, Errno::NOTSUP] {
            let (_temp, mut action) = fixture();
            let admitted = action.object.identity.clone();
            let mut scans = 0;
            let mut renames = 0;
            let result = remove_with_scan(
                &mut action,
                |_| {
                    scans += 1;
                    if scans == 2 {
                        Err(std::io::Error::from_raw_os_error(
                            Errno::BUSY.raw_os_error(),
                        ))
                    } else {
                        Ok(())
                    }
                },
                |source, destination| {
                    renames += 1;
                    if renames == if rollback { 2 } else { 1 } {
                        Err(std::io::Error::from_raw_os_error(errno.raw_os_error()))
                    } else {
                        rename_noreplace(source, destination)
                    }
                },
            );
            assert!(result.is_err());
            let (remaining, absent) = if rollback {
                (&action.quarantine, &action.object.path)
            } else {
                (&action.object.path, &action.quarantine)
            };
            assert_eq!(Identity::read(remaining).unwrap(), admitted);
            assert!(!absent.exists());
            assert_eq!(renames, if rollback { 2 } else { 1 });
            assert!(!action.done);
        }
    }
}
