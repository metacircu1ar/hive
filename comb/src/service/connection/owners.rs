//! Observe service producers during admission and while admitted handlers drain.
use super::{Arc, Error, Result, State, queries};
use tokio::task::JoinHandle;

#[cfg(test)]
mod tests;

pub(super) struct Owners {
    backfill: Option<JoinHandle<Result<()>>>,
    query: Option<JoinHandle<()>>,
    stopping: bool,
}
impl Owners {
    pub(super) fn start(
        state: &Arc<State>,
        receiver: tokio::sync::mpsc::Receiver<queries::Job>,
    ) -> Self {
        Self {
            backfill: state
                .model
                .as_ref()
                .map(|_| tokio::spawn(state.clone().backfill())),
            query: Some(tokio::spawn(queries::serve(state.clone(), receiver))),
            stopping: false,
        }
    }
    pub(super) async fn next(&mut self, state: &State) -> Result<()> {
        tokio::select! {
            result = wait(&mut self.backfill) => {
                self.backfill.take();
                match result {
                    Ok(result) => {
                        if result.is_err() { state.health.fence(); }
                        result
                    },
                    Err(error) if self.stopping && error.is_cancelled() => Ok(()),
                    Err(cause) => { state.health.fence(); Err(super::super::join_error(cause)) },
                }
            },
            result = wait(&mut self.query) => {
                self.query.take();
                state.health.fence();
                Err(match result {
                    Ok(()) => Error::query_worker_stopped(),
                    Err(cause) => super::super::join_error(cause),
                })
            },
        }
    }
    pub(super) fn stop_backfill(&mut self) {
        self.stopping = true;
        if let Some(worker) = &self.backfill {
            worker.abort();
        }
    }
    pub(super) async fn stop(&mut self, state: &State) -> Result<()> {
        self.stop_backfill();
        if let Some(query) = &self.query {
            query.abort();
        }
        let backfill = if let Some(worker) = self.backfill.take() {
            match worker.await {
                Ok(result) => result,
                Err(error) if error.is_cancelled() => Ok(()),
                Err(error) => Err(super::super::join_error(error)),
            }
        } else {
            Ok(())
        };
        let query = if let Some(query) = self.query.take() {
            match query.await {
                // The sender lives in State: only explicit cancellation is a
                // normal stop. serve may have swallowed a fenced worker panic.
                Ok(()) => Err(Error::query_worker_stopped()),
                Err(error) if error.is_cancelled() => Ok(()),
                Err(error) => Err(super::super::join_error(error)),
            }
        } else {
            Ok(())
        };
        if backfill.is_err() || query.is_err() {
            state.health.fence();
        }
        match backfill {
            Ok(()) => query,
            Err(error) => Err(super::super::auth::with_cleanup(error, query)),
        }
    }
}

async fn wait<T>(
    owner: &mut Option<JoinHandle<T>>,
) -> std::result::Result<T, tokio::task::JoinError> {
    match owner {
        Some(owner) => owner.await,
        None => std::future::pending().await,
    }
}
impl Drop for Owners {
    fn drop(&mut self) {
        if let Some(query) = &self.query {
            query.abort();
        }
        if let Some(worker) = &self.backfill {
            worker.abort();
        }
    }
}
