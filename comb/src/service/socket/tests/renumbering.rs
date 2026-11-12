use super::*;

#[test]
fn stale_receipts_match_renumbered_objects_but_never_replacements_or_corruption() {
    for (pointer, allowed) in [
        ("/parent/device", true),
        ("/root/node/device", true),
        ("/entry/node/device", true),
        ("/parent/inode", false),
        ("/root/node/inode", false),
        ("/entry/node/inode", false),
        ("/entry/ctime", false),
        ("/entry/ctime_nsec", false),
        ("/entry/node/device-negative", false),
    ] {
        let (dir, path) = fixture();
        let prior = abandoned(dir.path(), &path);
        let file = receipt_path(&path);
        let mut value = serde_json::to_value(prior).unwrap();
        if pointer.ends_with("negative") {
            value["entry"]["node"]["device"] = (-1).into();
        } else {
            let field = value.pointer_mut(pointer).unwrap();
            *field = serde_json::json!(field.as_u64().unwrap() ^ 1);
        }
        let bytes = serde_json::to_vec(&value).unwrap();
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .write(&file, &bytes)
            .unwrap();
        let lease = Lease::acquire(dir.path(), path.clone());
        assert_eq!(lease.is_ok(), allowed, "{pointer}");
        // Acquiring a lease never normalizes or overwrites the retained receipt.
        assert_eq!(fs::read(&file).unwrap(), bytes);
        drop(lease);
        if allowed {
            let server = Server::open(
                dir.path(),
                Options {
                    socket: Some(path.clone()),
                    ..Options::default()
                },
            )
            .unwrap();
            drop(server);
            assert!(!path.exists());
        } else {
            assert!(path.exists());
        }
    }
}
