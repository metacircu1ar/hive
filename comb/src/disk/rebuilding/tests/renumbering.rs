use super::*;

#[test]
fn pending_rebuild_matches_saved_devices_and_retains_replaced_inodes() {
    for (object, field, allowed) in [
        ("parent", "device", true),
        ("file", "device", true),
        ("parent", "inode", false),
        ("file", "inode", false),
    ] {
        let root = fixture(false);
        let admin = root.path().join(".comb");
        let disk = Disk::acquire(root.path(), false).unwrap();
        let intent = disk.begin_rebuild(false).unwrap();
        let mut value = serde_json::to_value(intent).unwrap();
        drop(disk);
        let identity = if object == "file" {
            &mut value["files"][0]
        } else {
            &mut value["parent"]
        };
        identity[field] = serde_json::json!(identity[field].as_u64().unwrap() ^ 1);
        let bytes = serde_json::to_vec(&value).unwrap();
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .write(admin.join(INTENT), &bytes)
            .unwrap();
        let opened = Comb::open(root.path());
        assert_eq!(opened.is_ok(), allowed, "{object}/{field}");
        drop(opened);
        if allowed {
            recover(root.path());
        } else {
            assert_eq!(fs::read(admin.join(INTENT)).unwrap(), bytes);
        }
    }
}
