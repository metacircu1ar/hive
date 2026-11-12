//! Typed declarations, queried without an embedding model.
pub use comb_protocol::graph::*;
mod diagnostics;
pub use crate::relations::Issue as ExtractionIssue;
pub use diagnostics::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Shared cancellation for blocking graph work. Cancelling never mutates pages.
#[derive(Clone, Default)]
pub(crate) struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
pub(crate) struct CancelOnDrop(pub Cancellation);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
