//! Retry only an explicit native timeout of the read-only account lookup.
use super::{Duration, Error, Instant, REQUEST_TIMEOUT, Response, Result, Rpc, outgoing, protocol};
use crate::runtime::provider::error::ACK_TIMEOUT;
use std::error::Error as _;

const RETRY_BACKOFF: Duration = Duration::from_millis(250);

#[derive(Debug, thiserror::Error)]
#[error("Provider workspace routing lookup timed out.")]
pub(super) struct RoutingTimeout;

impl Rpc {
    pub(super) fn read_account(&mut self) -> Result<Response> {
        self.read_account_until(self.effective_deadline(Instant::now() + REQUEST_TIMEOUT))
    }

    fn read_account_until(&mut self, deadline: Instant) -> Result<Response> {
        let mut retried = false;
        loop {
            if Instant::now() >= deadline {
                return Err(Error::provider_timeout(ACK_TIMEOUT));
            }
            let result = self.call_until(outgoing::account_read(), deadline);
            if self.uncertain() {
                return result.and_then(|_| Err(protocol()));
            }
            match result {
                Err(error)
                    if !retried
                        && error
                            .source()
                            .is_some_and(<dyn std::error::Error>::is::<RoutingTimeout>)
                        && RETRY_BACKOFF < deadline.saturating_duration_since(Instant::now()) =>
                {
                    retried = true;
                    tracing::warn!(name: "provider.account_lookup_retry",
                        pid = self.pid(),
                        attempt = 1_u64,
                        code = error.code(),
                        decision = "retry_read_only_timeout");
                    std::thread::sleep(RETRY_BACKOFF);
                }
                result => return result,
            }
        }
    }
}

#[cfg(test)]
mod tests;
