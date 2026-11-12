use super::*;
use std::os::unix::fs::symlink;

#[test]
fn preview_notices_a_dangling_pending_journal_without_following_or_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let prefix = temp.path().join("prefix");
    let part = crate::tests::fixture(temp.path(), "hive");
    publish::install(&prefix, temp.path(), &[part], |_| Ok(())).unwrap();
    let manager = prefix.join("lib/hive");
    let before = publish::current(&manager).unwrap();
    let pending = manager.join("pending-install.json");
    let missing = temp.path().join("missing-journal");
    symlink(&missing, &pending).unwrap();
    let error = remove_with(
        &prefix,
        &["hive".into()],
        true,
        &mut crate::lifecycle::Inert,
        |_| Ok(()),
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("Pending installation"));
    assert_eq!(std::fs::read_link(pending).unwrap(), missing);
    assert!(!missing.exists());
    assert_eq!(publish::current(&manager).unwrap(), before);
    assert!(prefix.join("bin/hivectl").is_file());
}
