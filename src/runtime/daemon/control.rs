//! Independently scheduled, bounded control connections with owned shutdown.
use super::super::{
    Shared,
    api::{self, Request, RequestEnvelope},
    hub, invalid_request, provider,
    reader::Reader,
};
use crate::error::{Error, Result};
use serde::Deserialize;
use serde_json::Value;
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::UnixListener,
    sync::oneshot,
    task::{JoinHandle, JoinSet},
};

mod wire;

// Maximum concurrent control connections admitted by the daemon.
const CONTROL_CLIENT_LIMIT: usize = 64;
// Delay after a listener accept error before retrying.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);
// How long a control connection may take to send its request line.
const SERVER_READ_TIMEOUT: Duration = Duration::from_secs(5);
// How long a control connection may take to accept its response.
const SERVER_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

// One diagnostic child at a time; retain the permit through physical cleanup,
// even if the requesting control connection disappears.
static SANDBOX_PROBES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

pub(super) struct Started {
    pub stop: oneshot::Sender<()>,
    pub task: JoinHandle<Result<()>>,
}

pub(super) fn start(
    listener: UnixListener,
    shared: Shared,
    reader: Reader,
    pool: provider::Pool,
    shutdown: Arc<AtomicBool>,
) -> Started {
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut clients = JoinSet::new();
        let result = ClientAcceptor {
            shared: &shared,
            reader: &reader,
            pool: &pool,
            clients: &mut clients,
            accept_failures: std::sync::atomic::AtomicU64::new(0),
        }
        .run(&listener, stopped)
        .await;
        // Viewers' long-polls end now, so joining their handlers cannot delay shutdown.
        shared.wake.live.stop();
        // Close the kernel backlog before waiting for accepted clients. Keep their
        // Shared/pool ownership until every handler has finished its response.
        drop(listener);
        finish(result, &mut clients, &shutdown, &shared.wake).await
    });
    Started { stop, task }
}

async fn finish(
    result: Result<()>,
    clients: &mut JoinSet<()>,
    shutdown: &AtomicBool,
    wake: &hub::Wake,
) -> Result<()> {
    if let Err(error) = &result {
        wake.failed(error);
        shutdown.store(true, Ordering::Relaxed);
        wake.all();
    }
    while let Some(joined) = clients.join_next().await {
        handler_join(joined);
    }
    result
}

struct ClientAcceptor<'a> {
    shared: &'a Shared,
    reader: &'a Reader,
    pool: &'a provider::Pool,
    clients: &'a mut JoinSet<()>,
    accept_failures: std::sync::atomic::AtomicU64,
}
impl Drop for ClientAcceptor<'_> {
    fn drop(&mut self) {
        let count = self.accept_failures.load(Ordering::Relaxed);
        if count != 0 {
            tracing::info!(name: "control.accept_failures_total", count = count);
        }
    }
}
impl ClientAcceptor<'_> {
    async fn run(
        &mut self,
        listener: &UnixListener,
        mut stopped: oneshot::Receiver<()>,
    ) -> Result<()> {
        loop {
            let capacity = has_client_capacity(self.clients);
            tokio::select! {
                biased;
                _ = &mut stopped => return Ok(()),
                accepted = listener.accept(), if capacity => {
                    if self.accepted(accepted, &mut stopped).await? {
                        return Ok(());
                    }
                },
                joined = self.clients.join_next(), if !self.clients.is_empty() => {
                    if let Some(joined) = joined { handler_join(joined); }
                }
            }
        }
    }

    async fn accepted(
        &mut self,
        accepted: std::io::Result<(tokio::net::UnixStream, tokio::net::unix::SocketAddr)>,
        stopped: &mut oneshot::Receiver<()>,
    ) -> Result<bool> {
        match accepted {
            Ok((stream, _)) => {
                let shared = self.shared.clone();
                let pool = self.pool.clone();
                let reader = self.reader.clone();
                self.clients.spawn(async move {
                    serve_client(shared, reader, pool, stream).await;
                });
                Ok(false)
            }
            Err(error) if retryable(&error) => {
                if let Some(count) = crate::observability::sample(&self.accept_failures) {
                    tracing::warn!(name: "control.accept_backoff",
                         error = &error as &(dyn std::error::Error + 'static),
                         count = count,
                         decision = "retry_after_backoff");
                }
                tokio::select! {
                    _ = stopped => Ok(true),
                    () = tokio::time::sleep(ACCEPT_BACKOFF) => Ok(false),
                }
            }
            Err(error) => Err(Error::state_io_from(error)),
        }
    }
}

fn retryable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::Interrupted | std::io::ErrorKind::ConnectionAborted
    ) || crate::os::descriptors_exhausted(error)
}

pub(super) struct Supervision {
    pub supervisor: Result<()>,
    pub control: Option<Result<()>>,
}

/// A control failure requests shutdown but must never cancel a partially awaited
/// admission or claim flush. Consume the control handle exactly once.
pub(super) async fn supervise(
    supervisor: impl Future<Output = Result<()>>,
    control: &mut JoinHandle<Result<()>>,
    shutdown: &AtomicBool,
    uncertain: &AtomicBool,
    wake: &hub::Wake,
) -> Supervision {
    tokio::pin!(supervisor);
    tokio::select! {
        result = &mut supervisor => {
            if let Err(error) = &result { wake.failed(error); }
            Supervision { supervisor: result, control: None }
        },
        joined = control => control_finished(supervisor, joined, shutdown, uncertain, wake).await,
    }
}

async fn control_finished(
    supervisor: impl Future<Output = Result<()>>,
    joined: std::result::Result<Result<()>, tokio::task::JoinError>,
    shutdown: &AtomicBool,
    uncertain: &AtomicBool,
    wake: &hub::Wake,
) -> Supervision {
    let result = outcome(joined, uncertain);
    if let Err(error) = &result {
        wake.failed(error);
    }
    shutdown.store(true, Ordering::Relaxed);
    wake.all();
    let supervisor = supervisor.await;
    if let Err(error) = &supervisor {
        wake.failed(error);
    }
    Supervision {
        supervisor,
        control: Some(result),
    }
}

pub(super) async fn join(
    task: JoinHandle<Result<()>>,
    consumed: Option<Result<()>>,
    uncertain: &AtomicBool,
) -> Result<()> {
    match consumed {
        Some(result) => result,
        None => outcome(task.await, uncertain),
    }
}

fn outcome(
    joined: std::result::Result<Result<()>, tokio::task::JoinError>,
    uncertain: &AtomicBool,
) -> Result<()> {
    joined.unwrap_or_else(|error| {
        // Unwinding the owner's JoinSet aborts handlers; their joins were not
        // confirmed. Preserve ownership evidence even after best-effort cleanup.
        uncertain.store(true, Ordering::Relaxed);
        Err(Error::daemon_stopping("Control task did not finish normally.").with_source(error))
    })
}

fn has_client_capacity(clients: &mut tokio::task::JoinSet<()>) -> bool {
    // JoinSet retains completed tasks until collected. Only live requests consume
    // slots, even if acceptance wins repeatedly over the completion branch.
    while let Some(joined) = clients.try_join_next() {
        handler_join(joined);
    }
    clients.len() < CONTROL_CLIENT_LIMIT
}

fn handler_join(joined: std::result::Result<(), tokio::task::JoinError>) {
    if let Err(error) = joined {
        let code = if error.is_panic() {
            "panicked"
        } else {
            "cancelled"
        };
        let task_id = error.id().to_string();
        tracing::warn!(name: "control.handler_failed",
             task_id = task_id,
             thread = "control_handler",
             code = code,
             error = &error as &(dyn std::error::Error + 'static),
             decision = "retain_siblings");
    }
}

async fn serve_client(
    shared: Shared,
    reader: Reader,
    pool: provider::Pool,
    stream: tokio::net::UnixStream,
) {
    use tokio::io::AsyncWriteExt;
    let (read, mut write) = stream.into_split();
    let Handled { result, request_id } = ClientRequest {
        shared: &shared,
        reader: &reader,
        pool: &pool,
    }
    .read(read)
    .await;
    let response = wire::encode(result, request_id);
    let data = format!("{response}\n");
    match tokio::time::timeout(SERVER_WRITE_TIMEOUT, write.write_all(data.as_bytes())).await {
        Ok(Ok(())) => (),
        Ok(Err(error)) => crate::observability::lost_reply(
            crate::observability::ReplyOwner::Control,
            Some(&error),
        ),
        Err(error) => crate::observability::lost_reply(
            crate::observability::ReplyOwner::Control,
            Some(&error),
        ),
    }
}

struct IncomingRequest {
    id: String,
    args: RequestEnvelope,
}
impl IncomingRequest {
    fn cleanup_request_id(&self) -> Option<String> {
        let mutating = match &self.args.request {
            Request::Gc(args) => args.dry_run.copied() != Some(true),
            Request::Purge(args) => args.dry_run.copied() != Some(true),
            _ => false,
        };
        mutating.then(|| self.id.clone())
    }
}
struct Handled {
    result: Result<wire::Output>,
    request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlInput {
    protocol: u32,
    #[serde(default)]
    id: api::Input<String>,
    #[serde(default)]
    args: Value,
}

async fn read_request(read: tokio::net::unix::OwnedReadHalf) -> Result<IncomingRequest> {
    let line = tokio::time::timeout(
        SERVER_READ_TIMEOUT,
        super::super::control_frame::Frame::Request.read_async(read),
    )
    .await
    .map_err(|cause| invalid_request().with_source(cause))??;
    let request_data: Value =
        serde_json::from_str(&line).map_err(|cause| invalid_request().with_source(cause))?;
    // The current outer protocol is strict; operation argument handling retains
    // its existing exact raw fingerprint and actionable validation semantics.
    if !request_data.is_object() {
        return Err(invalid_request());
    }
    let request_data: ControlInput =
        serde_json::from_str(&line).map_err(|cause| invalid_request().with_source(cause))?;
    if !crate::compatibility::CONTROL_RANGE.contains(request_data.protocol) {
        return Err(Error::IncompatibleProtocol {
            message: format!(
                "Unsupported control protocol. Hive {} supports {} through {}; update the \
                CLI or daemon to a compatible build.",
                env!("CARGO_PKG_VERSION"),
                crate::compatibility::CONTROL_RANGE.minimum,
                crate::compatibility::CONTROL_RANGE.maximum
            )
            .into(),
            source: None,
        });
    }
    let identity_error = || {
        api::InputContext::Control.error(
            hive_input::Code::InvalidRequestId,
            "/id",
            "a request ID string of 1..128 UTF-8 bytes without control characters",
        )
    };
    let id = request_data
        .id
        .optional(api::InputContext::Control, "/id", "a request ID string")
        .map_err(|cause| identity_error().with_source(cause))?
        .ok_or_else(identity_error)?;
    hive_input::validate::request_id(id).map_err(|cause| identity_error().with_source(cause))?;
    let id = id.clone();
    let args = RequestEnvelope::new(request_data.args);
    Ok(IncomingRequest { id, args })
}
struct ClientRequest<'a> {
    shared: &'a Shared,
    reader: &'a Reader,
    pool: &'a provider::Pool,
}
impl ClientRequest<'_> {
    async fn read(&self, read: tokio::net::unix::OwnedReadHalf) -> Handled {
        let request = match read_request(read).await {
            Ok(request) => request,
            Err(error) => {
                return Handled {
                    result: Err(error),
                    request_id: None,
                };
            }
        };
        let request_id = request.cleanup_request_id();
        let result = self
            .execute(request.id, request.args, request_id.as_deref())
            .await;
        Handled { result, request_id }
    }
    async fn execute(
        &self,
        id: String,
        args: RequestEnvelope,
        cleanup_request_id: Option<&str>,
    ) -> Result<wire::Output> {
        let Self {
            shared,
            reader,
            pool,
        } = self;
        match &args.request {
            Request::SandboxProbe => {
                return sandbox_probe(reader, crate::config::Provider::Codex)
                    .await
                    .map(wire::Output::plain);
            }
            Request::ClaudeSandboxProbe => {
                return sandbox_probe(reader, crate::config::Provider::Claude)
                    .await
                    .map(wire::Output::plain);
            }
            Request::Live(live) => return live_items(shared, live).await.map(wire::Output::plain),
            Request::Tree(_) => return tree(shared, reader, args).await.map(wire::Output::plain),
            Request::Attach(_) => {
                return attach(shared, reader, args).await.map(wire::Output::plain);
            }
            Request::Usage(args) => return usage(pool, reader, args).map(wire::Output::plain),
            _ => {}
        }
        self.dispatch(id, args, cleanup_request_id).await
    }

    async fn dispatch(
        &self,
        id: String,
        args: RequestEnvelope,
        cleanup_request_id: Option<&str>,
    ) -> Result<wire::Output> {
        let Self {
            shared,
            reader,
            pool,
        } = self;
        let gc = matches!(&args.request, Request::Gc(g) if g.dry_run.copied() != Some(true));
        let purge = matches!(&args.request, Request::Purge(p) if p.dry_run.copied() != Some(true));
        let status = matches!(args.request, Request::Ping);
        let crate::runtime::reader::Reply { value } = if args.request.is_read_only() {
            reader.request(&hub::Caller::Human, args).await?
        } else {
            shared
                .dispatch(hub::Caller::Human, id, args)
                .await
                .map(|value| crate::runtime::reader::Reply { value })?
        };
        let cleanup_pending = if gc {
            Some(
                finish_purge_batch(shared, reader)
                    .await
                    .map_err(purge_cleanup_error)?,
            )
        } else {
            None
        };
        if purge {
            finish_purge(shared, reader)
                .await
                .map_err(purge_cleanup_error)?;
        }
        let runtime_resources = status.then(|| pool.snapshot());
        let native_cleanup = if let Some(key) = cleanup_request_id {
            native_cleanup_receipt(shared, key.to_owned()).await?
        } else {
            None
        };
        Ok(wire::Output::core(
            value,
            api::ControlReply {
                request_id: None,
                cleanup_pending,
                runtime_resources,
                native_cleanup,
            },
        ))
    }
}

fn purge_cleanup_error(error: Error) -> Error {
    Error::purge_cleanup_pending(
        "Agent records were purged but output or native-session cleanup is incomplete. \
        Retry with the returned request_id.",
    )
    .with_source(error)
}

async fn native_cleanup_receipt(
    shared: &Shared,
    key: String,
) -> Result<Option<api::NativeCleanup>> {
    shared
        .with(move |core| {
            let report = core.store().native_cleanup_receipt(&key)?;
            let pending = core.store().native_cleanup_pending(&key)?;
            Ok(report.map(|mut report| {
                if pending && report.native_history != api::NativeHistory::Retained {
                    report.native_history = api::NativeHistory::Partial;
                }
                report
            }))
        })
        .await
        .map_err(purge_cleanup_error)
}

async fn sandbox_probe(
    reader: &Reader,
    provider: crate::config::Provider,
) -> Result<crate::native_sandbox::Report> {
    use crate::config::Provider;
    use crate::native_sandbox::{Outcome, Report};
    if !crate::native_sandbox::configured_for(reader.config(), provider) {
        return Ok(Report::new(
            Outcome::NotConfigured,
            None,
            match provider {
                Provider::Codex => "codex_not_configured",
                Provider::Claude => "claude_not_configured",
            },
        ));
    }
    let selected = match provider {
        Provider::Codex => reader.probe_binary(),
        Provider::Claude => reader.claude_probe_binary(),
    };
    let binary = match selected {
        Ok(binary) => binary,
        Err(error) => {
            tracing::warn!(name: "sandbox.provider_unavailable",
                 phase = "daemon",
                 code = error.code(),
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "return_unavailable_diagnostic");
            return Ok(Report::new(
                Outcome::Unavailable,
                None,
                "provider_unavailable",
            ));
        }
    };
    let Ok(permit) = SANDBOX_PROBES.try_acquire() else {
        return Ok(Report::new(Outcome::Busy, Some(binary), "probe_busy"));
    };
    // No native work runs on the reader, writer or asynchronous executor thread.
    let report = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        crate::native_sandbox::run_for(&binary, provider)
    })
    .await
    .map_err(|cause| {
        Error::state_io("Native sandbox diagnostic did not finish.").with_source(cause)
    })?;
    Ok(report)
}

/// A long-poll wait: 0..=25000 ms, defaulting to the longest wait.
fn wait(input: &api::Input<u64>, context: api::InputContext) -> Result<Duration> {
    const EXPECTED: &str = "an integer from 0 through 25000 milliseconds";
    let maximum = u64::try_from(crate::runtime::live::MAX_WAIT.as_millis()).unwrap_or(u64::MAX);
    match input.optional(context, "/wait_ms", EXPECTED)?.copied() {
        None => Ok(crate::runtime::live::MAX_WAIT),
        Some(ms) if ms <= maximum => Ok(Duration::from_millis(ms)),
        Some(_) => Err(context.error(hive_input::Code::InvalidValue, "/wait_ms", EXPECTED)),
    }
}

/// Wait outside the reader and writer, holding one of the long-poll permits.
async fn live_items(shared: &Shared, args: &api::LiveArgs) -> Result<crate::runtime::live::Page> {
    use api::InputContext as Op;
    const CURSOR: &str = "a cursor returned by attach or live";
    const LIMIT: &str = "an integer from 1 through 512";
    let hub = &shared.wake.live;
    let target = args.target.required(Op::Live, "/target")?;
    let after = *args
        .after
        .optional(Op::Live, "/after", CURSOR)?
        .ok_or_else(|| Op::Live.error(hive_input::Code::MissingArgument, "/after", CURSOR))?;
    let wait = wait(&args.wait_ms, Op::Live)?;
    let limit = match args.limit.optional(Op::Live, "/limit", LIMIT)?.copied() {
        None => crate::runtime::live::MAX_PAGE_ITEMS,
        Some(limit) => usize::try_from(limit)
            .ok()
            .filter(|limit| (1..=crate::runtime::live::MAX_PAGE_ITEMS).contains(limit))
            .ok_or_else(|| Op::Live.error(hive_input::Code::InvalidValue, "/limit", LIMIT))?,
    };
    let _permit = hub.admit_if(!wait.is_zero())?;
    hub.next(target, after, wait, limit).await
}

/// The quota usage each login's provider last reported, from memory. A named
/// login must be configured.
fn usage(
    pool: &provider::Pool,
    reader: &Reader,
    args: &api::UsageArgs,
) -> Result<api::UsageReply<Vec<provider::usage::Report>>> {
    let auth = args
        .auth
        .optional(api::InputContext::Usage, "/auth", "a login profile name")?;
    if let Some(auth) = auth {
        reader.config().profile(auth)?;
    }
    let reports = pool.usage.reports(auth.map(String::as_str));
    Ok(api::UsageReply { usage: reports })
}

/// Wait for a tree change, then read rows from one reader snapshot. The version
/// is taken first, so a change during the snapshot wakes the next poll at once.
async fn tree(
    shared: &Shared,
    reader: &Reader,
    args: RequestEnvelope,
) -> Result<api::TreeReply<api::Response>> {
    use api::InputContext as Op;
    let Request::Tree(tree) = &args.request else {
        return Err(invalid_request());
    };
    let after = tree
        .after
        .optional(Op::Tree, "/after", "a version returned by tree")?
        .copied();
    let wait = wait(&tree.wait_ms, Op::Tree)?;
    super::super::read_api::tree_selection(tree)?;
    let version = {
        let _permit = shared
            .wake
            .live
            .admit_if(after.is_some() && !wait.is_zero())?;
        shared.wake.live.tree_after(after, wait).await?
    };
    let crate::runtime::reader::Reply { value } = reader.request(&hub::Caller::Human, args).await?;
    Ok(api::TreeReply {
        version,
        tree: value,
    })
}

/// Open the live channel before the snapshot, so no item falls between them.
async fn attach(
    shared: &Shared,
    reader: &Reader,
    args: RequestEnvelope,
) -> Result<api::AttachReply<api::Response>> {
    let Request::Attach(attach) = &args.request else {
        return Err(invalid_request());
    };
    let target = attach
        .target
        .required(api::InputContext::Attach, "/target")?
        .to_owned();
    let (live, created) = shared.wake.live.open(&target)?;
    match reader.request(&hub::Caller::Human, args).await {
        Ok(crate::runtime::reader::Reply { value }) => Ok(api::AttachReply {
            live,
            snapshot: value,
        }),
        Err(error) => {
            if created {
                shared.wake.live.discard(&target, live.generation);
            }
            Err(error)
        }
    }
}

async fn finish_purge(shared: &Shared, reader: &Reader) -> Result<()> {
    while finish_purge_batch(shared, reader).await? {}
    Ok(())
}

/// Remove at most one committed batch. GC clients yield between cleanup requests.
async fn finish_purge_batch(shared: &Shared, reader: &Reader) -> Result<bool> {
    let attachment_pending = shared.cleanup_attachments().await?;
    // Retain serialization through the unlink and writer COMMIT, including if
    // the requesting client disconnects. Cleanup counts then acknowledge once.
    let gate = std::sync::Arc::new(shared.purge_cleanup_gate.clone().lock_owned().await);
    let (dir, ids, native, owner) = shared
        .with(|core| {
            let super::super::store::ArtifactDeletions { dir, ids } =
                core.store().pending_artifact_deletions()?;
            Ok((
                dir,
                ids,
                core.store().pending_native_deletions()?,
                core.daemon_lock().clone(),
            ))
        })
        .await?;
    if ids.is_empty() && native.is_empty() {
        return Ok(attachment_pending);
    }
    // Fetch precedes the barrier: every older artifact read finishes before unlink.
    reader.barrier().await?;
    let blocking_gate = gate.clone();
    let removed = tokio::task::spawn_blocking(move || {
        // Blocking jobs retain ownership even if their control handler is cancelled.
        let _owner = owner;
        let _gate = blocking_gate;
        if !ids.is_empty() {
            super::super::store::unlink_artifacts(&dir, &ids)?;
        }
        let (native, result) = super::super::store::unlink_native_files(native);
        Ok::<_, Error>((ids, native, result))
    })
    .await
    .map_err(|error| {
        Error::state_io("Artifact cleanup did not finish; retry the purge request.")
            .with_source(error)
    })??;
    let acknowledged = shared
        .with_retained(gate, move |core| {
            core.store().acknowledge_artifact_deletions(&removed.0)?;
            core.store().acknowledge_native_deletions(removed.1)?;
            Ok(attachment_pending
                || !core.store().pending_artifact_deletions()?.ids.is_empty()
                || core.store().has_native_deletions()?)
        })
        .await;
    crate::observability::settle(
        removed.2,
        acknowledged.as_ref().map(|_| ()).map_err(Clone::clone),
        "native_cleanup_ack",
    )?;
    acknowledged
}

#[cfg(test)]
// Test fixtures fail immediately on setup/assertion errors.
mod tests;
