use super::*;

thread_local! {
    static REPLACE_DURING_DIGEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
pub(in crate::runtime::steering_canary::proof) fn before_digest(path: &Path) -> Result<()> {
    if REPLACE_DURING_DIGEST.replace(false) {
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .write(path, b"\x7fELFreplaced during digest")
            .map_err(crate::publication::error)?;
    }
    Ok(())
}

#[test]
fn renumbered_proof_rejects_binary_replacement_during_digest_read() {
    let (root, binary, proof) = fixture();
    let mut value = serde_json::to_value(proof).unwrap();
    value["binary"]["device"] = serde_json::json!(value["binary"]["device"].as_u64().unwrap() ^ 1);
    write(
        &path(root.path()),
        Some(serde_json::from_value(value).unwrap()),
    )
    .unwrap();
    REPLACE_DURING_DIGEST.set(true);
    let candidate = load(root.path(), &binary).unwrap();
    assert_eq!(
        candidate.selection(None, "low").reason(),
        SteeringObservation::ProofStaleBinaryChanged
    );
    assert!(!REPLACE_DURING_DIGEST.get());
}

#[test]
fn saved_binary_device_can_change_without_rewriting_proof_or_weakening_live_checks() {
    for defect in [
        "device", "inode", "length", "digest", "modified", "changed", "adapter",
    ] {
        let (root, binary, proof) = fixture();
        let mut value = serde_json::to_value(proof).unwrap();
        value["binary"]["device"] =
            serde_json::json!(value["binary"]["device"].as_u64().unwrap() ^ 1);
        match defect {
            "inode" | "length" => {
                value["binary"][defect] =
                    serde_json::json!(value["binary"][defect].as_u64().unwrap() ^ 1);
            }
            "modified" | "changed" => value["binary"][defect][0] = 0.into(),
            "digest" => value["digest"] = hash(b"not the executable").into(),
            "adapter" => value["adapter"] = 0.into(),
            _ => {}
        }
        let proof: Proof = serde_json::from_value(value).unwrap();
        write(&path(root.path()), Some(proof.clone())).unwrap();
        let before = std::fs::read(path(root.path())).unwrap();
        let candidate = load(root.path(), &binary).unwrap();
        assert_eq!(
            candidate
                .selection(Some(&proof.model), &proof.effort)
                .mode()
                .is_some(),
            defect == "device",
            "{defect}"
        );
        assert_eq!(std::fs::read(path(root.path())).unwrap(), before);
        if defect == "device" {
            candidate.check_launch().unwrap();
            assert!(
                candidate
                    .selection(Some("other-model"), &proof.effort)
                    .mode()
                    .is_none()
            );
            std::fs::write(&binary, b"\x7fELFchanged-native").unwrap();
            assert!(candidate.check_launch().is_err());
            assert!(
                load(root.path(), &binary)
                    .unwrap()
                    .selection(Some(&proof.model), &proof.effort)
                    .mode()
                    .is_none()
            );
        }
    }
}
