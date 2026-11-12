//! Connection admission, frame handling and bounded shutdown.
use super::{
    Arc, BufReader, Duration, Error, Reply, Request, Result, Semaphore, Server, State,
    UnixListener, framing, input, limits,
    observations::{self, Loss},
    queries,
};
use std::future::Future;
use tokio::{net::UnixStream, sync::OwnedSemaphorePermit, task::JoinSet};

mod owners;
use owners::Owners;

struct Workers {
    owners: Owners,
    handlers: JoinSet<()>,
}

#[cfg(test)]
mod tests;

impl Server {
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<()> {
        self.run_with_deadline(shutdown, limits::REQUEST).await
    }

    pub(super) async fn run_with_deadline(
        self,
        shutdown: impl Future<Output = ()>,
        deadline: Duration,
    ) -> Result<()> {
        let Self {
            state,
            listener,
            queries,
            _socket: mut socket_guard,
        } = self;
        let listener = UnixListener::from_std(listener)?;
        let mut workers = Workers {
            owners: Owners::start(&state, queries),
            handlers: JoinSet::new(),
        };
        let result = accept_loop(&listener, &state, &mut workers, shutdown, deadline).await;
        drop(listener);
        let drained = shutdown_handlers(&state, &mut workers).await;
        let result = match result {
            Ok(()) => drained,
            Err(error) => Err(super::auth::with_cleanup(error, drained)),
        };
        state.observations.finish();
        let cleanup = socket_guard.cleanup();
        match result {
            Ok(()) => cleanup,
            Err(error) => Err(super::auth::with_cleanup(error, cleanup)),
        }
    }
}

async fn accept_loop(
    listener: &UnixListener,
    state: &Arc<State>,
    workers: &mut Workers,
    shutdown: impl Future<Output = ()>,
    deadline: Duration,
) -> Result<()> {
    let Workers { owners, handlers } = workers;
    // Independent frame headroom remains available while semantic slots wait.
    let frames = Arc::new(Semaphore::new(limits::FRAME_SLOTS));
    tokio::pin!(shutdown);
    let mut backoff = tokio::time::Instant::now();
    let mut failures = 0_u64;
    loop {
        tokio::select! {
            () = &mut shutdown => return Ok(()),
            result = owners.next(state) => result?,
            completed = handlers.join_next(), if !handlers.is_empty() => {
                if let Some(result) = completed {
                    observations::joined(result, "connection");
                }
            },
            accepted = async {
                tokio::time::sleep_until(backoff).await;
                listener.accept().await
            } => {
                match accepted {
                    Err(error) if transient_accept(&error) => {
                        failures = failures.saturating_add(1);
                        if failures.is_power_of_two() {
                            tracing::warn!(name: "comb.accept_retry", count = failures,
                                error = &error as &(dyn std::error::Error + 'static),
                                decision = "backoff");
                        }
                        backoff = tokio::time::Instant::now() + limits::ACCEPT_RETRY;
                    },
                    accepted => admit(accepted, state, &frames, handlers, deadline)?,
                }
            },
        }
    }
}

fn transient_accept(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::Interrupted | std::io::ErrorKind::ConnectionAborted
    ) || matches!(error.raw_os_error(), Some(libc::EMFILE | libc::ENFILE))
}

fn admit(
    accepted: std::io::Result<(UnixStream, tokio::net::unix::SocketAddr)>,
    state: &Arc<State>,
    frames: &Arc<Semaphore>,
    handlers: &mut JoinSet<()>,
    deadline: Duration,
) -> Result<()> {
    let (stream, _) = accepted?;
    let Ok(permit) = frames.clone().try_acquire_owned() else {
        state.observations.lost(Loss::FrameCapacity, None);
        return Ok(());
    };
    handlers.spawn(serve_connection(state.clone(), stream, permit, deadline));
    Ok(())
}

async fn serve_connection(
    state: Arc<State>,
    stream: UnixStream,
    permit: OwnedSemaphorePermit,
    deadline: Duration,
) {
    let _permit = permit;
    let mut stream = BufReader::new(stream);
    let result = match read_request(&mut stream).await {
        Ok(Some(request)) => handle_request(&state, request, deadline).await,
        Ok(None) => return,
        Err(error) => Err(error),
    };
    if let Err(error) = &result {
        error.log_diagnostic();
    }
    let reply = Reply::from_result(result);
    match tokio::time::timeout(
        limits::OUTPUT,
        framing::write(stream.get_mut(), &reply, framing::RESPONSE_BYTES),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let loss = if observations::peer_gone(&error) {
                Loss::PeerGone
            } else {
                Loss::ReplyFailure
            };
            state.observations.lost(loss, Some(&error));
        }
        Err(cause) => state.observations.lost(Loss::ReplyTimeout, Some(&cause)),
    }
}

async fn read_request(stream: &mut BufReader<UnixStream>) -> Result<Option<Request>> {
    match tokio::time::timeout(
        limits::FRAME_READ,
        framing::read(stream, framing::REQUEST_BYTES),
    )
    .await
    {
        Ok(Ok(Some(bytes))) => input::request(&bytes).map(Some),
        Ok(Ok(None)) => Ok(None),
        Ok(Err(error)) => Err(error),
        Err(cause) => Err(Error::request_frame_timeout().caused_by(cause)),
    }
}

async fn handle_request(
    state: &Arc<State>,
    request: Request,
    deadline: Duration,
) -> Result<serde_json::Value> {
    let id = request.operation.request_id().map(str::to_owned);
    let lost_reply = request.operation.lost_reply();
    match tokio::time::timeout(deadline, state.handle(request)).await {
        Ok(result) => result
            .and_then(super::api::Response::into_value)
            .map_err(|mut error| {
                error.request_id = id;
                error
            }),
        Err(cause) => Err(lost_reply.caused_by(cause)),
    }
}

async fn shutdown_handlers(state: &State, workers: &mut Workers) -> Result<()> {
    let Workers { owners, handlers } = workers;
    owners.stop_backfill();
    let mut failure = None;
    // Stop accepting first; admitted blocking jobs retain their own permits
    // after this bounded drain drops the client observations.
    let drained = tokio::time::timeout(limits::DRAIN, async {
        while !handlers.is_empty() {
            tokio::select! {
                result = handlers.join_next() => if let Some(result) = result {
                    observations::joined(result, "connection");
                },
                result = owners.next(state) => if let Err(error) = result {
                    if failure.is_none() { failure = Some(error); } else { error.log_diagnostic(); }
                },
            }
        }
    })
    .await;
    if let Err(cause) = drained {
        tracing::warn!(name: "comb.drain_timeout",
             count = handlers.len(),
             error = &cause as &(dyn std::error::Error + 'static),
             decision = "abort_observations");
    }
    handlers.abort_all();
    let cleanup = match tokio::time::timeout(limits::OWNER_STOP, async {
        let result = owners.stop(state).await;
        while let Some(result) = handlers.join_next().await {
            observations::joined(result, "connection");
        }
        result
    })
    .await
    {
        Ok(result) => result,
        Err(cause) => Err(Error::worker_failed().caused_by(cause)),
    };
    match failure {
        None => cleanup,
        Some(error) => Err(super::auth::with_cleanup(error, cleanup)),
    }
}
