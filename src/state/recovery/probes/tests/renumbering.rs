use super::*;

#[test]
fn probe_manifest_renumbering_requires_a_different_boot_and_same_inode() {
    let _fork = crate::os::fork_guard().unwrap();
    for (cross_boot, field, allowed) in [
        (true, "dev", true),
        (false, "dev", false),
        (true, "ino", false),
    ] {
        let (_temporary, _state, operation, dir) = preparation::fixture();
        drop(operation);
        let path = dir.join("identity.json");
        let mut value: serde_json::Value = read_json(&path).unwrap();
        let id = uuid::Uuid::parse_str(value["id"].as_str().unwrap()).unwrap();
        value["lock"][field] = serde_json::json!(value["lock"][field].as_u64().unwrap() ^ 1);
        if cross_boot {
            value["boot"] = uuid::Uuid::nil().to_string().into();
        }
        write_json(&path, &value).unwrap();
        let before = fs::read(&path).unwrap();
        assert_eq!(authorize(&dir, Some(id)).is_ok(), allowed);
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn saved_probe_removal_permit_survives_device_renumbering_only() {
    let _fork = crate::os::fork_guard().unwrap();
    for (object, field, allowed) in [
        ("tree", "device", true),
        ("parent", "device", true),
        ("credential_lock", "dev", true),
        ("tree", "inode", false),
        ("parent", "inode", false),
        ("credential_lock", "ino", false),
    ] {
        let (_temporary, _state, operation, dir) = preparation::fixture();
        drop(operation);
        let lock = Lock::acquire_existing(&dir.join("auth.lock")).unwrap();
        let paths = crate::state::recovery::probes::removal::Paths::new(&dir).unwrap();
        paths.permit(&lock).unwrap();
        drop(lock);
        let path = dir.with_file_name(format!(
            "{}.prune.json",
            dir.file_name().unwrap().to_str().unwrap()
        ));
        let mut value: serde_json::Value = read_json(&path).unwrap();
        value[object][field] = serde_json::json!(value[object][field].as_u64().unwrap() ^ 1);
        write_json(&path, &value).unwrap();
        let before = fs::read(&path).unwrap();
        assert_eq!(paths.remove().is_ok(), allowed, "{object}/{field}");
        assert_eq!(dir.exists(), !allowed);
        if !allowed {
            assert_eq!(fs::read(&path).unwrap(), before);
        }
    }
}
