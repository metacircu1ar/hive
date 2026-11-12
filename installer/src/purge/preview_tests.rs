//! Pending installation evidence must be noticed by the actual purge preview.
use super::*;
use clap::Parser;
use std::{ffi::OsStr, fs, os::unix::fs::symlink};

#[test]
fn purge_preview_refuses_a_dangling_pending_journal_without_changing_owned_links() {
    let temp = tempfile::tempdir().unwrap();
    let prefix = temp.path().join("prefix");
    let part = crate::tests::fixture(temp.path(), "hive");
    publish::install(&prefix, temp.path(), &[part], |_| Ok(())).unwrap();
    let manager = prefix.join("lib/hive");
    let before = publish::current(&manager).unwrap();
    let links = [
        "bin/hivectl",
        "bin/hive-daemon",
        "bin/hive-install",
        "share/hive",
    ]
    .map(|name| {
        let path = prefix.join(name);
        let target = fs::read_link(&path).unwrap();
        (path, target)
    });
    let pending = manager.join("pending-install.json");
    let missing = temp.path().join("missing-journal");
    symlink(&missing, &pending).unwrap();
    let args = Args::try_parse_from([
        OsStr::new("hive-install"),
        OsStr::new("--uninstall"),
        OsStr::new("--purge"),
        OsStr::new("--dry-run"),
        OsStr::new("--prefix"),
        prefix.as_os_str(),
    ])
    .unwrap();
    let error = run(&args).err().unwrap();
    assert!(
        error
            .to_string()
            .contains("Recover pending installation before purge preview")
    );
    assert_eq!(fs::read_link(pending).unwrap(), missing);
    assert!(!missing.exists());
    assert!(!prefix.join("lib").join(RECEIPT).exists());
    assert_eq!(publish::current(&manager).unwrap(), before);
    for (path, target) in links {
        assert_eq!(fs::read_link(path).unwrap(), target);
    }
}
