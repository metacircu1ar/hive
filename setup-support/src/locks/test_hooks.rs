//! One-shot descriptor-validation race placement in isolated thread-local fixtures.
use std::{cell::RefCell, path::Path};
type Hook = Box<dyn FnOnce(&Path) -> crate::Result<()>>;
thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }
struct Restore(Option<Hook>);
impl Drop for Restore {
    fn drop(&mut self) {
        HOOK.replace(self.0.take());
    }
}
pub(crate) fn with_hook<T>(
    hook: impl FnOnce(&Path) -> crate::Result<()> + 'static,
    run: impl FnOnce() -> T,
) -> T {
    let _restore = Restore(HOOK.replace(Some(Box::new(hook))));
    run()
}
pub(super) fn opened(path: &Path) -> crate::Result<()> {
    let hook = HOOK.with_borrow_mut(Option::take);
    if let Some(hook) = hook {
        hook(path)?;
    }
    Ok(())
}
