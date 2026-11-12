use super::*;
use std::os::unix::fs::PermissionsExt;
type Hook = Box<dyn Fn(&Path) -> std::io::Result<()>>;
thread_local! {
    static CREATED: std::cell::RefCell<Option<Hook>> = const { std::cell::RefCell::new(None) };
}
struct Reset(Option<Hook>);
impl Drop for Reset {
    fn drop(&mut self) {
        CREATED.set(self.0.take());
    }
}
pub(super) fn created(path: &Path) -> std::io::Result<()> {
    CREATED.with_borrow(|hook| hook.as_ref().map_or(Ok(()), |hook| hook(path)))
}
pub(crate) fn with_creation_hook<T>(
    hook: impl Fn(&Path) -> std::io::Result<()> + 'static,
    run: impl FnOnce() -> T,
) -> T {
    let _reset = Reset(CREATED.replace(Some(Box::new(hook))));
    run()
}

fn owned(parent: &Path) -> Owned {
    Owned::new(
        tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)
            .unwrap(),
    )
    .unwrap()
}
#[test]
fn secondary_and_io_wrapped_publication_evidence_retains_the_whole_preparation() {
    let temp = tempfile::tempdir().unwrap();
    for secondary in [false, true] {
        let directory = owned(temp.path());
        let path = directory.path().to_owned();
        let retained = path.join("retained");
        fs::write(&retained, b"evidence").unwrap();
        let publication = atomic_fs::Error::new(std::io::Error::other("sync failed"), true)
            .with_retained(&retained);
        let error: Error = if secondary {
            super::super::failed("primary".into(), "secondary", Err(publication.into()))
        } else {
            std::io::Error::other(Error::from(publication)).into()
        };
        assert!(directory.finish::<()>(Err(error)).is_err());
        assert_eq!(fs::read(retained).unwrap(), b"evidence");
    }
}
#[test]
fn substituted_preparation_is_retained_without_deleting_either_identity() {
    let temp = tempfile::tempdir().unwrap();
    let directory = owned(temp.path());
    let path = directory.path().to_owned();
    let retained = temp.path().join("original");
    fs::write(path.join("evidence"), b"original").unwrap();
    fs::rename(&path, &retained).unwrap();
    fs::create_dir(&path).unwrap();
    fs::write(path.join("foreign"), b"foreign").unwrap();
    drop(directory);
    assert_eq!(fs::read(path.join("foreign")).unwrap(), b"foreign");
    assert_eq!(fs::read(retained.join("evidence")).unwrap(), b"original");
}

#[test]
fn unwinding_retains_private_preparation_contents() {
    let temp = tempfile::tempdir().unwrap();
    let directory = owned(temp.path());
    let path = directory.path().to_owned();
    fs::write(path.join("evidence"), b"retain on unwind").unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _owner = directory;
        panic!("owned fixture unwind");
    }));
    assert!(result.is_err());
    assert_eq!(
        fs::read(path.join("evidence")).unwrap(),
        b"retain on unwind"
    );
}

#[test]
fn refused_inner_constructor_retains_enclosing_owner_and_preserves_primary_error() {
    let temp = tempfile::tempdir().unwrap();
    let outer = owned(temp.path());
    let root = outer.path().to_owned();
    let inner = tempfile::tempdir_in(&root).unwrap();
    let path = inner.path().to_owned();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let error = Owned::new(inner).err().unwrap();
    let error = super::super::failed("primary failure".into(), "admission", Err(error));
    let error = outer.finish::<()>(Err(error)).unwrap_err();
    assert!(error.to_string().starts_with("primary failure"));
    assert!(path.is_dir());
    assert!(root.is_dir());
}
