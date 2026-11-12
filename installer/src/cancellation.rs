//! Preserve installer primary/publication evidence around the shared native scope.
use crate::{Result, publication};
use hive_setup_support::runner::Signals;
use std::{io, process::Command};

pub(super) struct Cancellation(Signals);
impl Cancellation {
    pub(super) fn new() -> Result<Self> {
        Ok(Self(Signals::new()?))
    }
    pub(super) fn cancelled(&self) -> bool {
        self.0.cancelled()
    }
    pub(super) fn configure_child(&self, command: &mut Command) {
        self.0.configure_child(command);
    }
    pub(super) fn finish<T>(mut self, result: Result<T>) -> Result<T> {
        let cleanup = self.0.restore().map_err(Into::into);
        let result = result.and_then(|value| {
            if self.cancelled() {
                Err(io::Error::from(io::ErrorKind::Interrupted).into())
            } else {
                Ok(value)
            }
        });
        match result {
            Ok(value) => {
                cleanup?;
                Ok(value)
            }
            Err(primary) => Err(publication::failed(
                primary,
                "Cannot restore setup signals",
                cleanup,
            )),
        }
    }
}
