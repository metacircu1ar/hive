//! Own daemon service startup, supervision, and the ordered shutdown of all producers.
use super::{
    Arc, AtomicBool, Core, DaemonSignals, Error, MAINTENANCE_INTERVAL, Ordering, Queries, Result,
    Supervisor, UnixListener, control, daemon_signals, finish_store, hub, join_daemon_workers,
    provider, reader,
};
use std::collections::HashSet;

pub(super) struct ServiceInput<'a> {
    pub core: Core,
    pub queries: Queries,
    pub settings: &'a crate::config::Runtime,
    pub shutdown: &'a Arc<AtomicBool>,
    pub cleanup_uncertain: &'a Arc<AtomicBool>,
    pub recovered: Vec<provider::RecoveredReservation>,
}
// Field drop order mirrors the original reverse local order on startup failure.
pub(super) struct Services {
    auth_task: tokio::task::JoinHandle<Result<()>>,
    #[cfg(feature = "comb")]
    collector_task: tokio::task::JoinHandle<Result<()>>,
    pool: provider::Pool,
    reader_thread: std::thread::JoinHandle<Result<()>>,
    reader: reader::Reader,
    preflight: super::super::preflight::Owner,
    writer: std::thread::JoinHandle<Result<()>>,
    shared: hub::Hub,
    wake: Arc<hub::Wake>,
    // The reaper outlives producers even if startup/supervision unwinds.
    reaper: super::reaper::Owner,
}
impl ServiceInput<'_> {
    pub fn start(self) -> Result<Services> {
        let Self {
            core,
            queries,
            settings,
            shutdown,
            cleanup_uncertain,
            recovered,
        } = self;
        let wake = Arc::new(hub::Wake::default());
        let reaper =
            super::reaper::Owner::start(wake.clone(), shutdown.clone(), cleanup_uncertain.clone())?;
        let hub::StartedHub {
            hub: shared,
            writer,
            preflight,
        } = hub::Hub::start(core, wake.clone(), cleanup_uncertain.clone())?;
        let reader::Started {
            reader,
            thread: reader_thread,
        } = start_reader(
            queries,
            &shared,
            shutdown.clone(),
            cleanup_uncertain.clone(),
        );
        let pool = provider::Pool::new(
            settings.clone(),
            shared.clone(),
            cleanup_uncertain.clone(),
            shutdown.clone(),
            recovered,
        );
        let auth_task = start_auth_owner(
            shared.clone(),
            pool.clone(),
            shutdown.clone(),
            cleanup_uncertain.clone(),
        );
        #[cfg(feature = "comb")]
        let collector_task =
            start_collector_owner(shared.clone(), shutdown.clone(), cleanup_uncertain.clone());
        Ok(Services {
            auth_task,
            #[cfg(feature = "comb")]
            collector_task,
            pool,
            reader_thread,
            reader,
            preflight,
            writer,
            shared,
            wake,
            reaper,
        })
    }
}

fn start_reader(
    queries: Queries,
    shared: &hub::Hub,
    shutdown: Arc<AtomicBool>,
    uncertain: Arc<AtomicBool>,
) -> reader::Started {
    let started = reader::Reader::start(
        queries,
        shared.published.clone(),
        shutdown,
        uncertain,
        shared.wake.clone(),
    );
    assert!(
        shared.reader.set(started.reader.downgrade()).is_ok(),
        "reader installed once"
    );
    started
}

#[cfg(feature = "comb")]
fn start_collector_owner(
    collector_hub: hub::Hub,
    collector_shutdown: Arc<AtomicBool>,
    uncertain: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<Result<()>> {
    let boundary = super::super::owner::Boundary::new(
        collector_hub.wake.clone(),
        collector_shutdown.clone(),
        uncertain,
    );
    tokio::spawn(async move {
        let result =
            super::super::collectors::run(collector_hub.clone(), collector_shutdown.clone()).await;
        boundary.finish(&result);
        result
    })
}

fn start_auth_owner(
    shared: hub::Hub,
    pool: provider::Pool,
    shutdown: Arc<AtomicBool>,
    uncertain: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<Result<()>> {
    let boundary =
        super::super::owner::Boundary::new(shared.wake.clone(), shutdown.clone(), uncertain);
    tokio::spawn(async move {
        let result = super::super::auth_fallback::run(shared.clone(), pool, shutdown.clone()).await;
        boundary.finish(&result);
        result
    })
}

fn join_reader(thread: std::thread::JoinHandle<Result<()>>, uncertain: &AtomicBool) -> Result<()> {
    thread.join().unwrap_or_else(|_| {
        tracing::error!(name: "reader.join_failed",
             thread = "reader",
             code = "panicked",
             decision = "retain_evidence");
        uncertain.store(true, Ordering::Relaxed);
        Err(Error::daemon_stopping(
            "Control reader did not finish normally.",
        ))
    })
}

struct SupervisorResources {
    maintenance: tokio::time::Interval,
    turns: Arc<tokio::sync::Semaphore>,
    starting: Arc<tokio::sync::Semaphore>,
    occupied: HashSet<String>,
    workers: tokio::task::JoinSet<String>,
    signals: DaemonSignals,
}
pub(super) struct DaemonRun {
    control_task: tokio::task::JoinHandle<Result<()>>,
    control_stop: tokio::sync::oneshot::Sender<()>,
    resources: SupervisorResources,
}
pub(super) struct FinishedDaemon {
    pub result: Result<()>,
    // Signal registrations and admission owners survive final diagnostics and owner cleanup.
    _resources: SupervisorResources,
    _wake: Arc<hub::Wake>,
}
pub(super) struct PreparedRun {
    listener: tokio::net::UnixListener,
    signals: DaemonSignals,
}
impl PreparedRun {
    pub fn prepare(listener: UnixListener) -> Result<Self> {
        let listener =
            tokio::net::UnixListener::from_std(listener).map_err(Error::state_io_from)?;
        let signals = daemon_signals()?;
        Ok(Self { listener, signals })
    }
    pub fn start(
        self,
        settings: &crate::config::Runtime,
        services: &Services,
        shutdown: &Arc<AtomicBool>,
    ) -> DaemonRun {
        let Self { listener, signals } = self;
        let Services {
            shared,
            reader,
            pool,
            ..
        } = services;
        let workers = tokio::task::JoinSet::new();
        let occupied = HashSet::<String>::new();
        let starting = Arc::new(tokio::sync::Semaphore::new(settings.max_starting));
        let turns = pool.turns.clone();
        let mut maintenance = tokio::time::interval(MAINTENANCE_INTERVAL);
        maintenance.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let control::Started {
            stop: control_stop,
            task: control_task,
        } = control::start(
            listener,
            shared.clone(),
            reader.clone(),
            pool.clone(),
            shutdown.clone(),
        );
        DaemonRun {
            resources: SupervisorResources {
                maintenance,
                turns,
                starting,
                occupied,
                workers,
                signals,
            },
            control_stop,
            control_task,
        }
    }
}
impl DaemonRun {
    pub async fn supervise(
        &mut self,
        services: &Services,
        settings: &crate::config::Runtime,
        shutdown: &AtomicBool,
        cleanup_uncertain: &AtomicBool,
    ) -> control::Supervision {
        let Services {
            shared, pool, wake, ..
        } = services;
        let SupervisorResources {
            signals,
            workers,
            occupied,
            starting,
            turns,
            maintenance,
        } = &mut self.resources;
        Supervisor {
            settings,
            shutdown,
            cleanup_uncertain,
            shared,
            pool,
            wake,
            sigint: &mut signals.sigint,
            sigterm: &mut signals.sigterm,
            sighup: &mut signals.sighup,
            workers,
            occupied,
            starting,
            turns,
            maintenance,
        }
        .supervise(&mut self.control_task)
        .await
    }
    pub async fn finish(
        self,
        services: Services,
        supervision: control::Supervision,
        shutdown: &AtomicBool,
        cleanup_uncertain: &AtomicBool,
    ) -> FinishedDaemon {
        let Services {
            reaper,
            #[cfg(feature = "comb")]
            collector_task,
            auth_task,
            pool,
            reader_thread,
            reader,
            preflight,
            writer,
            shared,
            wake,
        } = services;
        let Self {
            mut resources,
            control_stop,
            control_task,
        } = self;
        let control::Supervision {
            supervisor: loop_result,
            control: control_result,
        } = supervision;
        shutdown.store(true, Ordering::Relaxed);
        wake.all();
        crate::observability::reply(
            control_stop.send(()),
            crate::observability::ReplyOwner::Shutdown,
        );
        join_daemon_workers(&mut resources.workers, cleanup_uncertain).await;
        let control_result = control::join(control_task, control_result, cleanup_uncertain).await;
        #[cfg(not(feature = "comb"))]
        let collector_result = Ok(());
        #[cfg(feature = "comb")]
        let collector_result = collector_task
            .await
            .map_err(|cause| Error::internal_error("Collector owner failed.").with_source(cause))
            .and_then(|result| result);
        drop(reader);
        let reader_result = join_reader(reader_thread, cleanup_uncertain);
        let auth_result = auth_task
            .await
            .map_err(|cause| Error::internal_error("Deadline owner failed.").with_source(cause))
            .and_then(|result| result);
        shared.catalogs.stop().await;
        pool.shutdown().await;
        drop(pool);
        let preflight_result = preflight.finish();
        let orphan_result = reaper.finish();
        drop(shared);
        let writer_result = finish_store(writer, cleanup_uncertain);
        let result = super::logging::results([
            ("supervisor", loop_result),
            ("control", control_result),
            ("reader", reader_result),
            ("preflight", preflight_result),
            ("reaper", orphan_result),
            ("auth", auth_result),
            ("collector", collector_result),
            ("writer", writer_result),
        ]);
        FinishedDaemon {
            result: wake.failure().map_or(result, Err),
            _resources: resources,
            _wake: wake,
        }
    }
}
