//! Publish failed background ownership before channels or retained state disappear.
use super::hub::Wake;
use crate::error::{Error, Result};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) struct Boundary {
    wake: Arc<Wake>,
    shutdown: Arc<AtomicBool>,
    uncertain: Arc<AtomicBool>,
    armed: bool,
}
impl Boundary {
    /// Construct before spawning so cancellation before first poll is observed.
    pub(super) fn new(
        wake: Arc<Wake>,
        shutdown: Arc<AtomicBool>,
        uncertain: Arc<AtomicBool>,
    ) -> Self {
        Self {
            wake,
            shutdown,
            uncertain,
            armed: true,
        }
    }
    fn stop(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.wake.all();
    }
    pub(super) fn finish(mut self, result: &Result<()>) {
        if let Err(error) = result {
            self.wake.failed(error);
            self.stop();
        }
        self.armed = false;
    }
}
impl Drop for Boundary {
    fn drop(&mut self) {
        if self.armed {
            self.wake.failed(&Error::daemon_stopping(
                "Background owner did not finish normally.",
            ));
            self.uncertain.store(true, Ordering::Release);
            self.stop();
        }
    }
}

#[cfg(test)]
mod tests;
