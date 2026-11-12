use super::*;

#[test]
fn interrupted_prune_reuses_saved_devices_but_never_a_replaced_inode() {
    let _fork = crate::os::fork_guard().unwrap();
    for (object, field, allowed) in [
        ("provider", "device", true),
        ("work", "device", true),
        ("credential_lock", "dev", true),
        ("provider", "inode", false),
        ("work", "inode", false),
        ("credential_lock", "ino", false),
    ] {
        let fixture = Fixture::new();
        let failed = with_hook(
            |path, step| {
                if path.file_name().unwrap() == "provider" && step == Step::BeforePublish {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || fixture.prune(),
        );
        assert!(failed.is_err());
        let path = fixture.binding.directory.join("prune-intent.json");
        let mut value: Value = state::read_json(&path).unwrap();
        value[object][field] = serde_json::json!(value[object][field].as_u64().unwrap() ^ 1);
        state::write_json(&path, &value).unwrap();
        let before = fs::read(&path).unwrap();
        assert_eq!(fixture.prune().is_ok(), allowed, "{object}/{field}");
        if allowed {
            fixture.completed();
        }
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
