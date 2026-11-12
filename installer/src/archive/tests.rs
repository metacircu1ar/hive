use super::*;
use std::{
    cell::RefCell,
    io::{Seek, SeekFrom, Write},
};
type Hook = Box<dyn FnOnce(&Path) -> io::Result<()>>;
thread_local! {
    static COPYING: RefCell<Option<Hook>> = const { RefCell::new(None) };
    static VERIFIED: RefCell<Option<Hook>> = const { RefCell::new(None) };
}
struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        COPYING.set(None);
        VERIFIED.set(None);
    }
}
pub(super) fn during_copy(path: &Path) -> io::Result<()> {
    if let Some(hook) = COPYING.take() {
        hook(path)?;
    }
    Ok(())
}
pub(super) fn after_verification(path: &Path) -> io::Result<()> {
    if let Some(hook) = VERIFIED.take() {
        hook(path)?;
    }
    Ok(())
}

#[test]
fn extraction_consumes_only_verified_snapshot_after_original_changes_in_place() {
    let _reset = Reset;
    let input = tempfile::tempdir().unwrap();
    let assets = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    let part = crate::tests::fixture(input.path(), "hive");
    crate::tests::archive(assets.path(), input.path(), &part, None);
    VERIFIED.set(Some(Box::new(|path| {
        fs::write(path, b"changed after verification")
    })));
    let decoded = unpack(assets.path(), output.path(), "hive", &part.target).unwrap();
    decoded.check_files(&output.path().join("hive")).unwrap();
    assert_eq!(
        fs::read(assets.path().join(format!("hive-{}.tar.gz", part.target))).unwrap(),
        b"changed after verification"
    );
}

#[test]
fn in_place_change_during_copy_refuses_before_any_extracted_payload() {
    let _reset = Reset;
    let input = tempfile::tempdir().unwrap();
    let assets = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    let mut part = crate::tests::fixture(input.path(), "hive");
    let name = "share/hive/THIRD_PARTY_LICENSES";
    let bytes: Vec<_> = (0..200_000)
        .scan(0x1234_5678u32, |seed, _| {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 17;
            *seed ^= *seed << 5;
            Some(u8::try_from(*seed & 255).unwrap())
        })
        .collect();
    let path = input.path().join("hive").join(name);
    fs::write(&path, &bytes).unwrap();
    part.files.insert(
        name.into(),
        manifest::File {
            mode: 0o644,
            bytes: manifest::byte_count(bytes.len()),
            sha256: manifest::hash(&path).unwrap(),
        },
    );
    crate::tests::archive(assets.path(), input.path(), &part, None);
    COPYING.set(Some(Box::new(|path| {
        let mut file = fs::OpenOptions::new().write(true).open(path)?;
        file.seek(SeekFrom::End(-1))?;
        file.write_all(b"X")
    })));
    assert!(unpack(assets.path(), output.path(), "hive", &part.target).is_err());
    assert!(!output.path().join("hive").exists());
}

#[test]
fn substituted_snapshot_propagates_retention_through_actual_outer_staging_cleanup() {
    let _reset = Reset;
    let input = tempfile::tempdir().unwrap();
    let assets = tempfile::tempdir().unwrap();
    let staging = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let outer = publication::Owned::new(staging).unwrap();
    let root = outer.path().to_owned();
    let part = crate::tests::fixture(input.path(), "hive");
    crate::tests::archive(assets.path(), input.path(), &part, None);
    let original = root.join("original-snapshot");
    let replaced = std::rc::Rc::new(RefCell::new(None));
    let observed = replaced.clone();
    let destination = original.clone();
    let staging = root.clone();
    VERIFIED.set(Some(Box::new(move |_| {
        let snapshot = fs::read_dir(&staging)?
            .next()
            .ok_or_else(|| io::Error::other("snapshot absent"))??
            .path();
        fs::rename(&snapshot, &destination)?;
        fs::DirBuilder::new().mode(0o700).create(&snapshot)?;
        fs::write(snapshot.join("foreign"), b"retain replacement")?;
        *observed.borrow_mut() = Some(snapshot);
        Ok(())
    })));
    let result = unpack(assets.path(), &root, "hive", &part.target);
    assert!(result.is_err());
    assert!(outer.finish(result).is_err());
    let replacement = replaced.borrow();
    let replacement = replacement.as_ref().unwrap();
    assert!(original.join("verified.tar.gz").is_file());
    assert_eq!(
        fs::read(replacement.join("foreign")).unwrap(),
        b"retain replacement"
    );
    fs::remove_dir_all(root).unwrap();
}
