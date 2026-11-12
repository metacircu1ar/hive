use super::{Instant, MeasuredBatch, Result};
use std::time::Duration;

pub(super) struct Progress {
    inputs: usize,
    last: Instant,
    idle: bool,
    failures: u64,
}
impl Progress {
    pub fn new() -> Self {
        Self {
            inputs: 0,
            last: Instant::now(),
            idle: false,
            failures: 0,
        }
    }
    pub fn retry(&mut self, error: &crate::Error) {
        self.failures = self.failures.saturating_add(1);
        if self.failures.is_power_of_two() {
            tracing::warn!(name: "comb.backfill_retry",
                 code = error.code.as_str(),
                 error = error as &(dyn std::error::Error + 'static),
                 count = self.failures,
                 duration_ms = super::limits::BACKFILL_RETRY.as_millis(),
                 decision = "backoff");
        }
    }
    pub fn record(&mut self, result: &Result<Option<MeasuredBatch>>, empty: bool) {
        match result {
            Ok(Some(work)) => {
                self.idle = false;
                self.inputs = self.inputs.saturating_add(work.inputs);
                if self.last.elapsed() >= PROGRESS_INTERVAL {
                    tracing::info!(name: "comb.backfill_progress",
                         count = self.inputs,
                         elapsed_ms = self.last.elapsed().as_millis());
                    self.last = Instant::now();
                }
            }
            Ok(None) if empty && !self.idle => {
                self.idle = true;
                tracing::info!(name: "comb.backfill_caught_up", count = self.inputs);
            }
            // Yielding to queries is not an empty work queue.
            Ok(None) | Err(_) => (),
        }
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        tracing::info!(name: "comb.backfill_finished",
             count = self.inputs,
             attempt = self.failures,
             decision = "worker_exited");
    }
}

const PROGRESS_INTERVAL: std::time::Duration = Duration::from_secs(10);
