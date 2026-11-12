//! Daemon startup, supervision, control connections and ordered shutdown.
mod collection;
mod control;
mod lifecycle;
mod logging;
mod reaper;
use crate::state::daemon as owner;
mod startup;
mod supervisor;
use super::{
    Core, hub, invalid_request, provider, reader, socket_path,
    store::{Queries, Store},
};
use crate::{
    config::Config,
    error::{Error, Result},
    state::{self, Lock, State},
};
use owner::write_daemon_owner;
pub(crate) use owner::{check_daemon_owner, maintenance_lock, purge_lock};
use std::{
    fs,
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::UnixListener,
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
pub(super) use supervisor::Supervisor;

// Four executor threads keep local control work available during native I/O.
const RUNTIME_WORKER_THREADS: usize = 4;
// The control socket is accessible only to its owning user.
const CONTROL_SOCKET_MODE: u32 = 0o600;
// Interval between supervisor maintenance wakeups.
const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(60);
pub(crate) fn serve(config_path: PathBuf, root: PathBuf, binaries: super::Binaries) -> Result<()> {
    let owned = startup::OwnedDaemon::open(config_path, root, binaries)?;
    let _owner = owned.lock();
    let logger = logging::ProcessLog::start(owned.runtime(), owned.logging(), owned.lock())?;
    let result = run(owned);
    logger.finish(result)
}
/// Pure startup validation, before raw log retention or root admission mutates disk.
pub(crate) fn daemon_configuration(path: &Path) -> Result<(Config, hive_log::Config)> {
    let config = Config::load(path)?;
    let logging = logging::configuration(&config.logging)?;
    Ok((config, logging))
}
pub(crate) fn validate_logging() -> Result<()> {
    logging_configuration().map(|_| ())
}
pub(crate) fn logging_configuration() -> Result<hive_log::Config> {
    logging::configuration(&crate::config::Logging::default())
}
fn run(owned: startup::OwnedDaemon) -> Result<()> {
    // Drop the runtime before finishing diagnostics, so task teardown can still log.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(RUNTIME_WORKER_THREADS)
        .enable_all()
        .build()
        .map_err(Error::state_io_from)?;
    runtime.block_on(serve_async(owned))
}
fn recover_store(runtime: &Path, config: &Config) -> Result<Store> {
    #[cfg(feature = "comb")]
    let identities = startup::collector_identities(config);
    #[cfg(not(feature = "comb"))]
    let _ = config;
    let store = Store::open(&runtime.join("runtime.sqlite"))?;
    store.begin()?;
    #[cfg(feature = "comb")]
    store.configure_collectors(&config.comb_collectors, &identities)?;
    store.recover_agents()?;
    store.commit()?;
    tracing::info!(name: "recovery.agents", success = true);
    store.finish_artifact_deletions()?;
    tracing::info!(name: "recovery.artifact_deletions", success = true);
    store.finish_native_deletions()?;
    tracing::info!(name: "recovery.native_sessions", success = true);
    store.recover_attachments()?;
    tracing::info!(name: "recovery.attachments", success = true);
    // Recovery may touch the whole history; enable RAM temporaries only after it.
    store.use_memory_temporaries()?;
    Ok(store)
}
struct BoundControlSocket {
    socket: PathBuf,
    listener: UnixListener,
}

fn bind_control_socket(root: &Path) -> Result<BoundControlSocket> {
    let socket = socket_path(root);
    match socket.symlink_metadata() {
        Ok(meta) => {
            if !meta.file_type().is_socket() {
                return Err(Error::unsafe_state(
                    "The control socket path is not a socket.",
                ));
            }
            fs::remove_file(&socket).map_err(Error::state_io_from)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(Error::state_io_from(error)),
    }
    let listener = UnixListener::bind(&socket).map_err(Error::state_io_from)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(CONTROL_SOCKET_MODE))
        .map_err(Error::state_io_from)?;
    listener
        .set_nonblocking(true)
        .map_err(Error::state_io_from)?;
    Ok(BoundControlSocket { socket, listener })
}
fn recovery_capacity(core: &Core) -> Result<Vec<provider::RecoveredReservation>> {
    // Fallible collection preserves reservation order and stops at the first count error.
    core.state()
        .recovery_reservations()?
        .into_iter()
        .map(|record| {
            let slots = core
                .store()
                .provider_agent_count(&record.provider)?
                .min(core.config().runtime.max_loaded);
            Ok(provider::RecoveredReservation {
                reservation: record,
                slots,
                process_options: None,
            })
        })
        .collect::<Result<Vec<_>>>()
}
struct DaemonSignals {
    sigint: tokio::signal::unix::Signal,
    sigterm: tokio::signal::unix::Signal,
    sighup: tokio::signal::unix::Signal,
}

fn daemon_signals() -> Result<DaemonSignals> {
    let sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(Error::state_io_from)?;
    let sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(Error::state_io_from)?;
    let sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        .map_err(Error::state_io_from)?;
    Ok(DaemonSignals {
        sigint,
        sigterm,
        sighup,
    })
}
async fn serve_async(owned: startup::OwnedDaemon) -> Result<()> {
    // Destructure in acquisition order: the outer lock outlives all services.
    let startup::PreparedDaemon {
        runtime,
        daemon_lock: _daemon_lock,
        marker,
        queries,
        socket,
        listener,
        shutdown,
        cleanup_uncertain,
        settings,
        core,
        recovered,
    } = startup::PreparedDaemon::open(owned)?;
    let prepared_run = lifecycle::PreparedRun::prepare(listener)?;
    let services = lifecycle::ServiceInput {
        core,
        queries,
        settings: &settings,
        shutdown: &shutdown,
        cleanup_uncertain: &cleanup_uncertain,
        recovered,
    }
    .start()?;
    let mut run = prepared_run.start(&settings, &services, &shutdown);
    hive_setup_support::managed::ready();
    tracing::info!(name: "daemon.ready", success = true);
    let supervision = run
        .supervise(&services, &settings, &shutdown, &cleanup_uncertain)
        .await;
    let finished = run
        .finish(services, supervision, &shutdown, &cleanup_uncertain)
        .await;
    let cleanup = finish_daemon_shutdown(&socket, &marker, &runtime, &cleanup_uncertain);
    logging::results([("execution", finished.result), ("cleanup", cleanup)])
}

fn finish_store(
    writer: std::thread::JoinHandle<Result<()>>,
    cleanup_uncertain: &AtomicBool,
) -> Result<()> {
    // Only provider threads call blocking_with, and all have exited before this join.
    let result = writer.join().unwrap_or_else(|_| {
        tracing::error!(name: "writer.join_failed",
             thread = "writer",
             code = "panicked",
             decision = "retain_evidence");
        Err(Error::daemon_stopping(
            "Runtime writer did not finish normally.",
        ))
    });
    if result.is_err() {
        cleanup_uncertain.store(true, Ordering::Relaxed);
    }
    result
}

async fn join_daemon_workers(
    workers: &mut tokio::task::JoinSet<String>,
    cleanup_uncertain: &AtomicBool,
) {
    while let Some(done) = workers.join_next().await {
        match done {
            Ok(_) => {}
            Err(error) => {
                let task_id = error.id().to_string();
                tracing::error!(name: "worker.shutdown_join_failed",
                     task_id = task_id,
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "retain_evidence");
                cleanup_uncertain.store(true, Ordering::Relaxed);
            }
        }
    }
}

fn finish_daemon_shutdown(
    socket: &Path,
    marker: &Path,
    runtime: &Path,
    cleanup_uncertain: &AtomicBool,
) -> Result<()> {
    fs::remove_file(socket).map_err(Error::state_io_from)?;
    if cleanup_uncertain.load(Ordering::Relaxed) {
        return Err(Error::recovery_required(
            "Runtime shutdown could not be fully localized. Recovery records were \
                retained; stop the daemon and run auth recover for affected profiles.",
        ));
    }
    fs::remove_file(marker).map_err(Error::state_io_from)?;
    fs::File::open(runtime)
        .and_then(|f| f.sync_all())
        .map_err(Error::state_io_from)?;
    Ok(())
}

#[cfg(test)]
// Isolated daemon fixtures fail immediately.
mod failure_tests;
