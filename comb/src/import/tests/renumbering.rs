use super::*;

#[test]
fn completed_import_starts_fresh_when_a_source_with_untrusted_parent_no_longer_matches() {
    use std::os::unix::fs::PermissionsExt;
    for defect in ["device", "inode"] {
        let mut fixture = Fixture::new();
        let parent = tempfile::tempdir().unwrap();
        fixture.source = tempfile::tempdir_in(parent.path()).unwrap();
        for name in ["a", "b", "c"] {
            fs::write(fixture.source.path().join(format!("{name}.md")), name).unwrap();
        }
        let first = fixture.apply().unwrap();
        if defect == "device" {
            let mut value: serde_json::Value = serde_json::from_slice(&fixture.journal()).unwrap();
            value["batch"]["source"]["device"] =
                serde_json::json!(value["batch"]["source"]["device"].as_u64().unwrap() ^ 1);
            atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                .write(
                    fixture.root.path().join(".comb/import.json"),
                    serde_json::to_vec(&value).unwrap(),
                )
                .unwrap();
        } else {
            fs::rename(fixture.source.path(), parent.path().join("retained")).unwrap();
            fs::create_dir(fixture.source.path()).unwrap();
            for name in ["a", "b", "c"] {
                fs::write(fixture.source.path().join(format!("{name}.md")), name).unwrap();
            }
        }
        let permissions = fs::metadata(parent.path()).unwrap().permissions();
        fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o777)).unwrap();
        let result = fixture.apply();
        fs::set_permissions(parent.path(), permissions).unwrap();
        assert_ne!(result.unwrap().batch_id, first.batch_id);
        assert_eq!(fixture.comb.stats().unwrap().pages, 3);
    }
}

#[test]
fn unreadable_renumbering_anchor_keeps_the_pending_import_progress() {
    use std::os::unix::fs::PermissionsExt;
    let mut fixture = Fixture::new();
    let parent = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir_in(parent.path()).unwrap();
    for name in ["a", "b", "c"] {
        fs::write(source.path().join(format!("{name}.md")), name).unwrap();
    }
    fixture.source = source;
    let pending = fixture.pending(1);
    let mut value: serde_json::Value = serde_json::from_slice(&fixture.journal()).unwrap();
    value["batch"]["source"]["device"] =
        serde_json::json!(value["batch"]["source"]["device"].as_u64().unwrap() ^ 1);
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(
            fixture.root.path().join(".comb/import.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    let before = fixture.journal();
    // Only this fixture-owned parent is changed, never the host temp directory.
    let permissions = fs::metadata(parent.path()).unwrap().permissions();
    fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o777)).unwrap();
    let result = fixture.apply();
    fs::set_permissions(parent.path(), permissions).unwrap();
    let error = result.unwrap_err();
    assert!(error.file_committed);
    assert_eq!(error.import_progress.unwrap().batch_id, pending.batch_id);
    assert_eq!(fixture.journal(), before);
}

#[test]
fn pending_import_resumes_after_device_renumbering_but_keeps_content_binding() {
    for defect in ["device", "inode", "content", "prefix"] {
        let mut fixture = Fixture::new();
        fixture.pending(1);
        let mut value: serde_json::Value = serde_json::from_slice(&fixture.journal()).unwrap();
        value["batch"]["source"]["device"] =
            serde_json::json!(value["batch"]["source"]["device"].as_u64().unwrap() ^ 1);
        if defect == "inode" {
            value["batch"]["source"]["inode"] = 0.into();
        }
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .write(
                fixture.root.path().join(".comb/import.json"),
                serde_json::to_vec(&value).unwrap(),
            )
            .unwrap();
        if defect == "content" {
            fs::write(fixture.source.path().join("c.md"), "changed").unwrap();
        }
        let saved_device = value["batch"]["source"]["device"].clone();
        let mut fixture = fixture.reopen();
        let result = if defect == "prefix" {
            fixture
                .comb
                .import(fixture.source.path(), Some(&"elsewhere".parse().unwrap()))
        } else {
            fixture.apply()
        };
        assert_eq!(result.is_ok(), defect == "device", "{defect}");
        let after: serde_json::Value = serde_json::from_slice(&fixture.journal()).unwrap();
        assert_eq!(after["batch"]["source"]["device"], saved_device);
        if defect == "device" {
            assert_eq!(fixture.comb.stats().unwrap().pages, 3);
        }
    }
}
