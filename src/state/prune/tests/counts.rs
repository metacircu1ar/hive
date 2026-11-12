use super::{recovery::Fixture, *};
use crate::state::prune::counts::NativeData;
use crate::state::sync_tests::with_hook;
use atomic_fs::Step;
use std::{fs, io, os::unix::fs::symlink};

#[test]
fn unreadable_native_data_never_becomes_a_zero_count_or_an_admitted_deletion() {
    use std::os::unix::fs::PermissionsExt;
    if rustix::process::geteuid().as_raw() == 0 {
        return;
    }
    let _fork = crate::os::fork_guard().unwrap();
    let fixture = Fixture::new();
    let child = fixture.binding.directory.join("provider/restricted");
    fs::create_dir(&child).unwrap();
    fs::write(child.join("history"), b"keep").unwrap();
    fs::set_permissions(&child, fs::Permissions::from_mode(0o000)).unwrap();
    let result = fixture.prune();
    // Restore access before assertions so failure cannot strand the fixture.
    fs::set_permissions(&child, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read(child.join("history")).unwrap(), b"keep");
    assert!(!fixture.binding.directory.join("prune-intent.json").exists());
    fixture.prune().unwrap();
    fixture.completed();
}

#[test]
fn partial_prune_retains_captured_counts_and_never_follows_native_symlinks() {
    let _fork = crate::os::fork_guard().unwrap();
    let fixture = Fixture::new();
    let directory = &fixture.binding.directory;
    let external = fixture
        .state
        .root
        .parent()
        .unwrap()
        .join("external-history");
    fs::write(&external, b"retained external bytes").unwrap();
    symlink(&external, directory.join("provider/link")).unwrap();
    drop(std::os::unix::net::UnixListener::bind(directory.join("provider/native.sock")).unwrap());
    let before = NativeData::capture(directory).unwrap();
    assert_eq!(before.files, 4);
    assert_eq!(before.directories, 2);
    assert_eq!(before.symlinks, 1);
    assert_eq!(before.special_files, 1);
    assert!(before.bytes > 0);
    let selected = directory.clone();
    let error = with_hook(
        move |path, step| {
            if path.file_name().unwrap() == "provider" && step == Step::BeforeRemove {
                fs::remove_file(selected.join("provider.pruning/a"))?;
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            Ok(())
        },
        || fixture.prune(),
    )
    .unwrap_err();
    assert_eq!(super::recovery::native_cause(&error), Some(libc::EIO));
    assert!(!directory.join("prune-receipt.json").exists());
    fixture.prune().unwrap();
    let receipt: PruneReceipt =
        super::super::super::read_json(&directory.join("prune-receipt.json")).unwrap();
    assert_eq!(receipt.native_data_removed, before);
    assert_eq!(fs::read(external).unwrap(), b"retained external bytes");
    fixture.completed();
}
