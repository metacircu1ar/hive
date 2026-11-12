//! Observe bounded call tasks without releasing a successfully queued final reply.
use super::{JoinError, Message, Result, Server, mpsc};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

#[cfg(test)]
mod tests;

pub(super) struct Completed {
    key: String,
    generation: String,
    result: std::result::Result<(), JoinError>,
}
impl Server {
    pub(super) fn poll_owner(&mut self, cx: &mut Context<'_>) -> Poll<Completed> {
        for (key, active) in &mut self.active {
            let Some(task) = &mut active.task else {
                continue;
            };
            if let Poll::Ready(result) = Pin::new(task).poll(cx) {
                active.task.take();
                return Poll::Ready(Completed {
                    key: key.clone(),
                    generation: active.generation.clone(),
                    result,
                });
            }
        }
        Poll::Pending
    }
    pub(super) async fn owner_finished(
        &mut self,
        output: &mpsc::Sender<Message>,
        completed: Completed,
    ) -> Result<()> {
        if let Err(error) = completed.result {
            let task_id = error.id().to_string();
            tracing::warn!(name: "mcp.call_observer_failed", task_id = task_id,
                error = &error as &(dyn std::error::Error + 'static), decision = "retire_observer");
            if self
                .active
                .get(&completed.key)
                .is_some_and(|active| active.generation == completed.generation)
                && let Some(active) = self.active.remove(&completed.key)
            {
                super::enqueue(output, Message::error(Some(active.id.clone()), -32603,
                    "External call observer did not finish normally; delivery is not cancelled."),
                    true).await?;
            }
        }
        // A successful task sent its final Update; progress consumes that update
        // and releases the active slot. Keep ID fencing until it has done so.
        Ok(())
    }
}
