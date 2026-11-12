use super::*;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

#[test]
fn m5c_initialization_uses_owned_publication_proof_and_not_existing_root_presence() {
    for existing in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        if existing {
            drop(crate::disk::Disk::acquire(&root, true).unwrap());
        }
        let error = crate::disk::publication_hooks::with(
            |path, step| {
                if path.file_name() == Some(std::ffi::OsStr::new("index-population.json"))
                    && step == atomic_fs::Step::BeforePublish
                {
                    Err(std::io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || {
                if existing {
                    Comb::open(&root)
                } else {
                    Comb::init(&root)
                }
                .err()
                .unwrap()
            },
        );
        assert_eq!(error.file_committed, !existing);
        assert!(root.join(".comb/identity.json").is_file());
        assert!(!root.join(".comb/index-population.json").exists());
    }
}

#[test]
fn m5c_private_initialization_preparations_are_not_a_published_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let error = crate::disk::publication_hooks::with(
        |path, step| {
            if path.file_name() == Some(std::ffi::OsStr::new("identity.json"))
                && step == atomic_fs::Step::AfterPublish
            {
                Err(std::io::Error::from_raw_os_error(libc::EIO))
            } else {
                Ok(())
            }
        },
        || Comb::init(&root).err().unwrap(),
    );
    assert!(!error.file_committed);
    assert!(!root.join(".comb").exists());
}

#[test]
fn m5c_cleanup_and_stats_failures_report_only_actual_page_publication() {
    for externalized in [false, true] {
        for stats_failure in [false, true] {
            let (root, mut comb) = TestComb::new().unwrap().into_parts();
            for name in ["new", "stale"] {
                comb.put_markdown(
                    &format!("components/{name}").parse().unwrap(),
                    &markdown("Initial"),
                    None,
                    &fixture_write(&Actor::Human, name, None).unwrap(),
                )
                .unwrap();
            }
            let changed = root.path().join("components/new.md");
            if externalized {
                std::fs::write(&changed, markdown("External change")).unwrap();
            }
            let before = std::fs::read(&changed).unwrap();
            std::fs::remove_file(root.path().join("components/stale.md")).unwrap();
            let db = comb.index.test_connection();
            if stats_failure {
                db.authorizer(Some(|context: AuthContext<'_>| {
                    if matches!(
                        context.action,
                        AuthAction::Read {
                            table_name: "pages",
                            column_name: "unresolved_proposal"
                        }
                    ) {
                        Authorization::Deny
                    } else {
                        Authorization::Allow
                    }
                }))
                .unwrap();
            } else {
                db.execute_batch("CREATE TRIGGER fail_stale_delete BEFORE DELETE ON pages WHEN old.slug='components/stale' BEGIN SELECT RAISE(ABORT, 'injected cleanup failure'); END;").unwrap();
            }
            let error = comb.sync().unwrap_err();
            assert_eq!(error.file_committed, externalized);
            assert_eq!(std::fs::read(&changed).unwrap() != before, externalized);
            comb.index
                .test_connection()
                .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
                .unwrap();
            let current = std::fs::read_to_string(&changed).unwrap();
            assert_eq!(
                comb.index
                    .content_hash(&"components/new".parse().unwrap())
                    .unwrap(),
                Some(crate::values::ContentHash::digest(&current))
            );
            assert_eq!(
                comb.index
                    .content_hash(&"components/stale".parse().unwrap())
                    .unwrap()
                    .is_none(),
                stats_failure
            );
        }
    }
}
