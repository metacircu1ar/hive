use super::{Context, Exit, args, command::LocalCommand, limits, runtime};
use comb::{
    Comb, Result,
    embedding::{self, Config, Embedder, Engine},
    error::value,
    service::{self, auth::Connection},
};
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
struct Initialized {
    comb_id: comb::values::CombId,
    stats: comb::store::Stats,
}
#[derive(Serialize)]
struct Stopped {
    stopped: bool,
}

impl LocalCommand {
    pub(super) fn execute(self, context: &Context) -> Result<Exit> {
        match self {
            Self::Logs(args) => super::logs::run(context, args),
            Self::SetupMaintenance(options) => {
                Ok(Exit::Json(comb::maintenance::run(&context.comb, &options)?))
            }
            Self::Service(command, logging) => Ok(Exit::Json(value(
                comb::services::run_configured(&context.comb, &command, &logging)?,
            )?)),
            Self::Setup(options) => {
                let prepared = super::input::setup::offer_local(&context.comb, *options)?;
                let prepared = if prepared.options().local_embeddings {
                    prepared.with_progress(super::models::reporter()?)
                } else {
                    prepared
                };
                Ok(Exit::Json(value(
                    prepared.run(&context.comb, context.socket.as_deref())?,
                )?))
            }
            Self::Model(command) => super::models::execute(context, *command),
            Self::Version { json } => version(json),
            Self::Init { dir } => init(context, dir.as_deref()),
            Self::Upgrade { dry_run } => {
                Ok(Exit::Json(value(Comb::upgrade(&context.comb, dry_run)?)?))
            }
            Self::Serve { plan, args, .. } => {
                // Managed environment may override the level; validate before recovery.
                let config = args.logging.backend()?;
                let prepared = super::logging::preparation(config.clone(), || {
                    plan.ok_or_else(comb::Error::internal_serialization)?
                        .prepare()
                })?;
                serve(prepared, config)
            }
            Self::Mcp { connection } => mcp(connection),
            Self::Embed { args, config } => embed(context, args, config),
            Self::Rebuild => Ok(Exit::Json(value(Comb::rebuild(&context.comb)?.stats()?)?)),
        }
    }
}

fn version(json: bool) -> Result<Exit> {
    let info = comb::build_info();
    if json {
        Ok(Exit::Json(value(info)?))
    } else {
        Ok(Exit::Text(format!(
            "{} {} ({})\n",
            info.product,
            info.product_version,
            info.compiled_features.join(",")
        )))
    }
}

fn init(context: &Context, dir: Option<&Path>) -> Result<Exit> {
    let comb = Comb::init(dir.unwrap_or(&context.comb))?;
    Ok(Exit::Json(value(Initialized {
        comb_id: comb.identity(),
        stats: comb.stats()?,
    })?))
}

fn serve(mut prepared: service::Prepared, config: hive_log::Config) -> Result<Exit> {
    let launch = prepared.take_llama();
    let logging = super::serve_log::ProcessLog::start(&prepared, config)?;
    logging.finish((|| {
        // The managed server starts on this main thread, before the runtime exists.
        // Its monitor records a later failure for embedding work; Comb keeps
        // serving text search and nothing restarts the server.
        let llama = launch
            .map(|(launch, availability)| {
                let signals = hive_setup_support::runner::Signals::new()?;
                let result = launch.spawn(&|| signals.cancelled());
                let running = signals.finish(result)?;
                running.monitor(comb::llama::READY_DEADLINE, availability)
            })
            .transpose()?;
        run_prepared(prepared, llama)
    })())
}

fn run_prepared(prepared: service::Prepared, llama: Option<comb::llama::Running>) -> Result<Exit> {
    let rt = runtime()?;
    // Keep long preparation interruptible, then install before advertising readiness.
    let mut interrupt = {
        let _entered = rt.enter();
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?
    };
    let mut terminate = {
        let _entered = rt.enter();
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?
    };
    let server = prepared.bind()?;
    tracing::info!(name: "comb.ready", success = true);
    hive_setup_support::managed::ready();
    let result = rt.block_on(server.run(async {
        let signal = tokio::select! { _ = interrupt.recv() => libc::SIGINT,
        _ = terminate.recv() => libc::SIGTERM };
        tracing::info!(name: "comb.shutdown_signal",
             signal = signal,
             decision = "graceful_shutdown");
    }));
    rt.shutdown_timeout(limits::SHUTDOWN);
    // Stop and reap the server outside the runtime, after serving ended. A server
    // its monitor already reaped or stopped is never signalled again.
    let stopped = llama.map(comb::llama::Running::stop).transpose();
    result?;
    stopped?;
    Ok(Exit::Json(value(Stopped { stopped: true })?))
}

fn mcp(connection: Connection) -> Result<Exit> {
    let rt = runtime()?;
    let result = rt.block_on(service::mcp::run(
        connection,
        tokio::io::BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
    ));
    rt.shutdown_timeout(limits::SHUTDOWN);
    result?;
    // MCP stdout consists exclusively of JSON-RPC frames.
    Ok(Exit::Silent)
}

fn embed(context: &Context, args: args::Embed, config: Config) -> Result<Exit> {
    let mut engine = Engine::load(config)?;
    let mut comb = Comb::open_for_embedding(&context.comb, engine.space(), args.replace)?;
    comb.activate_engine(&mut engine, args.replace)?;
    if args.retry_failed {
        comb.retry_embedding_failures()?;
    }
    let mut backfill = embedding::Backfill::default();
    while backfill.offline_step(&mut comb, &mut engine)? {}
    Ok(Exit::Json(value(
        comb.index()?
            .embedding_coverage(comb::search::Filter::default())?,
    )?))
}
