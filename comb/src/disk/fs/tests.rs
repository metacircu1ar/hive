//! Parent-entry durability survives a failed creation and an existing-path retry.
use super::*;
use crate::disk::{Operation, Recording};
use std::sync::Arc;

#[test]
fn admin_admission_refuses_a_replaced_open_directory_without_following_its_link() {
    use hive_setup_support::checkpoints::Checkpoint;
    use std::sync::atomic::{AtomicBool, Ordering};
    for human in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        drop(Disk::acquire(temp.path(), true).unwrap());
        let admin = temp.path().join(".comb");
        let moved = temp.path().join("retained");
        let identity = fs::read(admin.join("identity.json")).unwrap();
        let selected = admin.clone();
        let destination = moved.clone();
        let fired = Arc::new(AtomicBool::new(false));
        let trigger = fired.clone();
        let refused = crate::disk::durable::hooks::with_checkpoint(
            move |path, step| {
                if path == selected
                    && step == Checkpoint::RecordOpened
                    && !trigger.swap(true, Ordering::SeqCst)
                {
                    fs::rename(&selected, &destination)?;
                    std::os::unix::fs::symlink(&destination, &selected)?;
                }
                Ok(())
            },
            || {
                if human {
                    Disk::human_authority(temp.path()).is_err()
                } else {
                    Disk::acquire(temp.path(), false).is_err()
                }
            },
        );
        assert!(refused && fired.load(Ordering::SeqCst));
        assert_eq!(fs::read_link(&admin).unwrap(), moved);
        assert_eq!(fs::read(moved.join("identity.json")).unwrap(), identity);
    }
}

#[test]
fn existing_directory_retries_the_parent_barrier_but_inspection_does_not() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("private");
    let recording = Recording::default();
    for _ in 0..2 {
        recording.reset();
        recording.fail(Operation::DirectorySync, 1);
        assert!(dir(&path, true, &recording).is_err());
        assert!(path.is_dir());
    }
    recording.reset();
    dir(&path, false, &recording).unwrap();
    assert!(recording.counts().events.is_empty());
    dir(&path, true, &recording).unwrap();
    assert_eq!(recording.counts().events, [Operation::DirectorySync]);
}

#[test]
fn acquisition_retries_an_existing_ancestor_before_creating_the_root() {
    let temporary = tempfile::tempdir().unwrap();
    let parent = temporary.path().join("ancestor");
    let root = parent.join("root");
    let recording = Arc::new(Recording::default());
    // Existing ancestor, pre-mkdir parent, new directory, then its parent entry.
    recording.fail(Operation::DirectorySync, 4);
    assert!(Disk::acquire_with(&root, true, recording.clone()).is_err());
    assert!(parent.is_dir());
    assert!(!root.exists());
    recording.reset();
    recording.fail(Operation::DirectorySync, 1);
    assert!(Disk::acquire_with(&root, true, recording.clone()).is_err());
    assert!(!root.exists());
    recording.reset();
    Disk::acquire_with(&root, true, recording).unwrap();
}

#[test]
fn initialization_accepts_an_existing_ancestor_below_a_traverse_only_parent() {
    use std::os::unix::fs::PermissionsExt;
    // Root bypasses this native permission denial; M6 runs this on a non-root host.
    if rustix::process::geteuid().as_raw() == 0 {
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let grandparent = temporary.path().join("traverse-only");
    let parent = grandparent.join("private");
    fs::create_dir(&grandparent).unwrap();
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&grandparent, fs::Permissions::from_mode(0o311)).unwrap();
    let denied = File::open(&grandparent).unwrap_err();
    let result = Disk::acquire(&parent.join("root"), true);
    fs::set_permissions(&grandparent, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(denied.kind(), std::io::ErrorKind::PermissionDenied);
    drop(result.unwrap());
}

#[test]
fn unavailable_new_directory_parent_barrier_precedes_mkdir() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("not-published");
    let recording = Recording::default();
    recording.fail(Operation::DirectorySync, 2);
    assert!(create_root(&path, &recording).is_err());
    assert!(!path.exists());
    create_root(&path, &recording).unwrap();
}

#[test]
fn journal_orders_nested_parents_before_the_intent_full_flush() {
    let temporary = tempfile::tempdir().unwrap();
    let recording = Arc::new(Recording::default());
    let disk = Disk::acquire_with(temporary.path(), true, recording.clone()).unwrap();
    let slug = "first/second/page".parse().unwrap();
    let mut page = crate::golden::page(crate::Actor::Human);
    disk.seal(&slug, &mut page).unwrap();
    recording.reset();
    disk.write(&slug, Some(&page), None, None).unwrap();
    let events = recording.counts().events;
    let first_flush = events
        .iter()
        .position(|op| *op == Operation::FullFlush)
        .unwrap();
    assert!(first_flush >= 2, "{events:?}");
    assert_eq!(&events[..2], &[Operation::Fsync, Operation::Fsync]);
    assert_eq!(
        disk.content(&slug).unwrap().unwrap(),
        page.render().unwrap()
    );
    disk.finish().unwrap();
}
