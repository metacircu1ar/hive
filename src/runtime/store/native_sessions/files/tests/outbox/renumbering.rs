use super::*;

#[test]
fn queued_deletion_restarts_with_renumbered_devices_but_keeps_inode_and_content_pins() {
    for defect in [
        "device",
        "inode",
        "bytes",
        "modified_seconds",
        "replacement",
        "parent",
    ] {
        let temp = crate::runtime::test_directory();
        let generation = uuid::Uuid::new_v4();
        let home = home(temp.path(), generation);
        let owned = write(&home, &rollout(SESSION));
        let store = open(temp.path());
        agent(&store, "worker", generation, SESSION);
        purge(&store, "purge", "worker", false, false);
        let (id, json): (i64, String) = store
            .query_row_cached("SELECT id,data FROM native_cleanup_files", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value["identity"]["device"] =
            serde_json::json!(value["identity"]["device"].as_u64().unwrap() ^ 1);
        if ["inode", "bytes", "modified_seconds"].contains(&defect) {
            value["identity"][defect] =
                serde_json::json!(value["identity"][defect].as_i64().unwrap() ^ 1);
        }
        store
            .execute_cached(
                "UPDATE native_cleanup_files SET data=?1 WHERE id=?2",
                rusqlite::params![serde_json::to_string(&value).unwrap(), id],
            )
            .unwrap();
        if defect == "replacement" {
            write(&home, &rollout(SESSION));
        }
        if defect == "parent" {
            fs::rename(home.join("sessions"), home.join("saved-sessions")).unwrap();
            symlink(home.join("saved-sessions"), home.join("sessions")).unwrap();
        }
        drop(store);
        let recovered = open(temp.path());
        recovered.finish_native_deletions().unwrap();
        let receipt = recovered.native_cleanup_receipt("purge").unwrap().unwrap();
        assert_eq!(
            receipt.native_sessions_deleted,
            u64::from(defect == "device"),
            "{defect}"
        );
        assert_eq!(owned.exists(), defect != "device");
    }
}
