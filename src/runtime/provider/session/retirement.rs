//! Keep shared Codex ownership until native unload is proven.
use super::{
    Cmd, Duration, Error, Instant, Ordering, OwnedSemaphorePermit, Response, Result, Session, mpsc,
    oneshot, outgoing, protocol, retirement_notice, unavailable,
};
use crate::runtime::provider::error::THREAD_UNLOAD_TIMEOUT;

// Retirement never releases ownership until active work and unload are proven.
const SESSION_INTERRUPT_GRACE: Duration = Duration::from_secs(10);
const SESSION_INTERRUPT_POLL: Duration = Duration::from_millis(25);
const LOADED_THREAD_LIMIT: u32 = 1024;
const SESSION_UNLOAD_GRACE: Duration = Duration::from_secs(180);
const SESSION_UNLOAD_POLL: Duration = Duration::from_secs(1);

impl Session {
    pub(super) async fn retire(
        &mut self,
        turn_permit: &mut Option<OwnedSemaphorePermit>,
    ) -> Result<bool> {
        let Some(thread) = self.thread.clone() else {
            return Err(protocol());
        };
        if let Some(turn) = self.active(&thread).await? {
            self.retire_active_turn(&thread, &turn).await?;
        }
        self.clean_terminals(&thread).await?;
        // The turn and its terminal work have stopped; pending physical unload
        // must not consume a turn slot while healthy siblings continue.
        drop(turn_permit.take());
        loop {
            match self.events.try_recv() {
                Ok(frame) if frame.value.has_id() => return Err(protocol()),
                Ok(_) => {} // Already observed by the RPC owner before cleanup.
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => return Err(unavailable()),
            }
        }
        let frame_value = self.call(outgoing::thread_unsubscribe(&thread)).await?;
        if !matches!(
            frame_value.status(),
            Some(
                super::super::protocol::UnsubscribeStatus::Unsubscribed
                    | super::super::protocol::UnsubscribeStatus::NotSubscribed
                    | super::super::protocol::UnsubscribeStatus::NotLoaded
            )
        ) {
            return Err(protocol());
        }
        let (tx, rx) = oneshot::channel();
        self.shard
            .command(Cmd::PrepareRetirement(thread.clone(), tx), true)
            .await?;
        rx.await
            .map_err(|cause| unavailable().with_source(cause))??;
        if !self.confirm_unloaded(&thread).await? {
            return Ok(false);
        }
        let (tx, rx) = oneshot::channel();
        self.shard.command(Cmd::Retire(thread, tx), true).await?;
        rx.await
            .map_err(|cause| unavailable().with_source(cause))??;
        Ok(true)
    }
    async fn retire_active_turn(&mut self, thread: &str, turn: &str) -> Result<()> {
        self.interrupt(thread, turn).await?;
        let deadline = Instant::now() + SESSION_INTERRUPT_GRACE;
        while self.active(thread).await?.is_some() {
            if Instant::now() >= deadline {
                return Err(Error::interrupt_timeout(
                    "Provider did not confirm interruption.",
                ));
            }
            // RPC independently tracks the matching completion while these
            // already-observed events drain from the bounded route.
            while self.events.try_recv().is_ok() {}
            tokio::time::sleep(SESSION_INTERRUPT_POLL).await;
        }
        self.observe_terminal_cleanup(thread, turn).await?;
        Ok(())
    }
    async fn confirm_unloaded(&mut self, thread: &str) -> Result<bool> {
        let deadline = tokio::time::Instant::now() + SESSION_UNLOAD_GRACE;
        loop {
            if self.pool.shutdown.load(Ordering::Relaxed) || self.shard.draining() {
                return Ok(false);
            }
            let Some(response) = self.loaded_during_retirement(deadline).await? else {
                return Ok(false);
            };
            let (loaded, cursor) = response.loaded_threads()?;
            if cursor.is_some() {
                // More than the configured maximum means this is not a complete
                // inventory and therefore cannot prove absence.
                return Err(protocol());
            }
            if !loaded.contains(&thread) {
                loop {
                    match self.events.try_recv() {
                        Ok(frame) if retirement_notice(&frame) => {}
                        Ok(_) => return Err(protocol()),
                        Err(mpsc::error::TryRecvError::Empty) => break,
                        Err(mpsc::error::TryRecvError::Disconnected) => return Ok(false),
                    }
                }
                return Ok(true);
            }
            // An already-unloaded sole conversation can leave a reusable
            // provider. Only fence it when native retention is actually seen.
            if self.pool.close_for_retirement(&self.shard, &self.agent) {
                return Ok(false);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::provider_timeout(THREAD_UNLOAD_TIMEOUT));
            }
            tokio::select! {
                () = tokio::time::sleep(SESSION_UNLOAD_POLL) => {},
                event = self.events.recv() => {
                    match event {
                        Some(frame) if retirement_notice(&frame) => {},
                        Some(_) => return Err(protocol()),
                        None => return Ok(false),
                    }
                }
            }
        }
    }

    async fn loaded_during_retirement(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> Result<Option<Response>> {
        let request = self.pending_call(outgoing::loaded_thread_page(LOADED_THREAD_LIMIT, None));
        tokio::pin!(request);
        loop {
            if self.pool.shutdown.load(Ordering::Relaxed) || self.shard.draining() {
                return Ok(None);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::provider_timeout(THREAD_UNLOAD_TIMEOUT));
            }
            tokio::select! {
                result = &mut request => return result.map(Some),
                () = tokio::time::sleep(SESSION_UNLOAD_POLL) => {},
                event = self.events.recv() => match event {
                    Some(frame) if retirement_notice(&frame) => {},
                    Some(_) => return Err(protocol()),
                    None => return Ok(None),
                },
            }
        }
    }
}
