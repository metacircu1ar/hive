use super::*;
use std::cell::Cell;

struct FileFailure(Cell<usize>);
impl atomic_fs::Barriers for FileFailure {
    fn sync_file(&self, _file: &fs::File) -> Result<()> {
        self.0.set(self.0.get() + 1);
        Err(io::Error::from_raw_os_error(libc::EIO))
    }
}
#[test]
fn engine_copy_uses_caller_file_barrier_before_any_named_backup_or_intent() {
    let temp = fixture();
    let root = temp.path().canonicalize().unwrap();
    let before = ["a.md", "b.md", "format"].map(|name| fs::read(root.join(name)).unwrap());
    let owner = || Ok(());
    let engine = Engine::new(&root, &root, Path::new("format"), &owner).unwrap();
    let failure = FileFailure(Cell::new(0));
    assert!(engine.apply(&Synthetic(1, 2), &failure).is_err());
    assert_eq!(failure.0.get(), 1);
    assert!(engine.pending().unwrap().is_none());
    assert!(engine.backups().unwrap().is_empty());
    for (name, bytes) in ["a.md", "b.md", "format"].into_iter().zip(before) {
        assert_eq!(fs::read(root.join(name)).unwrap(), bytes);
    }
}

struct ParentFailure(Cell<usize>);
impl atomic_fs::Barriers for ParentFailure {
    fn sync_directory(&self, _file: &fs::File) -> Result<()> {
        self.0.set(self.0.get() + 1);
        Err(io::Error::from_raw_os_error(libc::EIO))
    }
}
#[test]
fn engine_directory_preparation_uses_caller_parent_barriers_before_publication() {
    let temp = fixture();
    let root = temp.path().canonicalize().unwrap();
    let owner = || Ok(());
    let engine = Engine::new(&root, &root, Path::new("format"), &owner).unwrap();
    let failure = ParentFailure(Cell::new(0));
    assert_eq!(
        engine
            .apply(&Synthetic(1, 2), &failure)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EIO)
    );
    assert_eq!(failure.0.get(), 1);
    assert!(!root.join("backups").exists());
    assert!(engine.pending().unwrap().is_none());
    Synthetic::validate(&root, 1).unwrap();
}
