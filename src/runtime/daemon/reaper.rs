//! Retain Linux orphan reaping through all other producers' ordered shutdown.
use crate::error::{Error, Result};
use crate::runtime::hub::Wake;
#[cfg(target_os = "linux")]
use crate::runtime::owner::Boundary;
use std::sync::{Arc, atomic::AtomicBool, mpsc};

pub(super) struct Owner {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Owner {
    #[cfg_attr(
        target_os = "macos",
        expect(
            clippy::unnecessary_wraps,
            reason = "Linux thread creation is fallible; retain one lifecycle interface."
        )
    )]
    pub(super) fn start(
        wake: Arc<Wake>,
        shutdown: Arc<AtomicBool>,
        uncertain: Arc<AtomicBool>,
    ) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let (stop, receiver) = mpsc::channel();
            let thread = std::thread::Builder::new()
                .name("hive-reaper".into())
                .spawn(move || {
                    let boundary = Boundary::new(wake, shutdown, uncertain);
                    run(receiver);
                    boundary.finish(&Ok(()));
                })
                .map_err(Error::state_io_from)?;
            Ok(Self {
                stop: Some(stop),
                thread: Some(thread),
            })
        }
        #[cfg(target_os = "macos")]
        {
            drop((wake, shutdown, uncertain));
            Ok(Self {
                stop: None,
                thread: None,
            })
        }
    }
    pub(super) fn finish(mut self) -> Result<()> {
        self.close()
    }
    fn close(&mut self) -> Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        if let Some(stop) = self.stop.take() {
            // A disconnected owner has already exited; joining below retains its outcome.
            if stop.send(()).is_err() {
                tracing::debug!(name: "daemon.reaper_already_exited", decision = "join_owner");
            }
        }
        thread
            .join()
            .map_err(|_panic| Error::daemon_stopping("Background owner did not finish normally."))
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            tracing::warn!(name: "daemon.reaper_cleanup_failed", code = error.code(),
                error = &error as &(dyn std::error::Error + 'static),
                decision = "preserve_primary");
        }
    }
}

#[cfg(target_os = "linux")]
fn run(receiver: mpsc::Receiver<()>) {
    const INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
    let mut failures = 0_u64;
    while let Err(mpsc::RecvTimeoutError::Timeout) = receiver.recv_timeout(INTERVAL) {
        if let Err(error) = crate::os::reap_adopted() {
            failures = failures.saturating_add(1);
            if failures.is_power_of_two() {
                tracing::warn!(name: "daemon.reap_failed", count = failures,
                    error = &error as &(dyn std::error::Error + 'static), decision = "retry");
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn stop_interrupts_the_reaper_interval_and_joins_its_thread() {
        let wake = Arc::new(Wake::default());
        let shutdown = Arc::new(AtomicBool::new(false));
        let uncertain = Arc::new(AtomicBool::new(false));
        let started = std::time::Instant::now();
        let owner = Owner::start(wake.clone(), shutdown.clone(), uncertain.clone()).unwrap();
        owner.finish().unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(!shutdown.load(std::sync::atomic::Ordering::Acquire));
        assert!(!uncertain.load(std::sync::atomic::Ordering::Acquire));
        assert!(wake.failure().is_none());
    }
}
