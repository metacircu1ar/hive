//! Nested synchronous I/O budgets include submission and all descendant writes.
use super::{Duration, Error, Instant, Result, Rpc};
use crate::runtime::provider::error::ACK_TIMEOUT;

impl Rpc {
    pub(super) fn effective_deadline(&self, deadline: Instant) -> Instant {
        self.io_deadline
            .map_or(deadline, |outer| outer.min(deadline))
    }
    pub(super) fn io_expired(&self) -> bool {
        self.io_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }
    pub(super) fn check_io_deadline(&self) -> Result<()> {
        if self.io_expired() {
            Err(Error::provider_timeout(ACK_TIMEOUT))
        } else {
            Ok(())
        }
    }
    pub(super) fn wait_budget(&self, timeout: Duration) -> Duration {
        self.io_deadline.map_or(timeout, |deadline| {
            timeout.min(deadline.saturating_duration_since(Instant::now()))
        })
    }
    pub(super) fn with_deadline<T>(
        &mut self,
        deadline: Instant,
        operation: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        let previous = self.io_deadline;
        self.io_deadline = Some(self.effective_deadline(deadline));
        let result = self.check_io_deadline().and_then(|()| operation(self));
        self.io_deadline = previous;
        result
    }
}
