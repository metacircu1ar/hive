use super::*;

#[test]
fn exact_saved_root_keeps_its_existing_admission_without_new_parent_requirements() {
    let original = fixture();
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().canonicalize().unwrap().join("owned");
    fs::rename(original.path(), &root).unwrap();
    let owner = || Ok(());
    let engine = Engine::new(&root, &root, Path::new("format"), &owner).unwrap();
    let failure = Fail {
        root: root.clone(),
        file: "a.md",
        fired: AtomicBool::new(false),
    };
    assert!(engine.apply(&Synthetic(1, 2), &failure).is_err());
    let permissions = fs::metadata(parent.path()).unwrap().permissions();
    fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o777)).unwrap();
    let result = engine.recover(&Synthetic(1, 2), &atomic_fs::DefaultBarriers);
    fs::set_permissions(parent.path(), permissions).unwrap();
    result.unwrap();
    Synthetic::validate(&root, 2).unwrap();
}

#[test]
fn pending_migration_keeps_saved_device_and_still_refuses_root_replacement() {
    for (field, allowed) in [("root_device", true), ("root_inode", false)] {
        let temp = fixture();
        let root = temp.path().canonicalize().unwrap();
        let owner = || Ok(());
        let engine = Engine::new(&root, &root, Path::new("format"), &owner).unwrap();
        let failure = Fail {
            root: root.clone(),
            file: "a.md",
            fired: AtomicBool::new(false),
        };
        assert!(engine.apply(&Synthetic(1, 2), &failure).is_err());
        let path = root.join(PENDING);
        let mut value: serde_json::Value = storage::record(&path).unwrap();
        value[field] = serde_json::json!(value[field].as_u64().unwrap() ^ 1);
        files::write(&path, &files::encode(&value).unwrap(), true).unwrap();
        let before = fs::read(&path).unwrap();
        assert_eq!(
            engine
                .recover(&Synthetic(1, 2), &atomic_fs::DefaultBarriers)
                .is_ok(),
            allowed
        );
        if allowed {
            Synthetic::validate(&root, 2).unwrap();
            let history = engine.history().unwrap();
            assert_eq!(
                history.last().unwrap().root_device,
                value["root_device"].as_u64().unwrap()
            );
        } else {
            assert_eq!(fs::read(path).unwrap(), before);
        }
    }
}
