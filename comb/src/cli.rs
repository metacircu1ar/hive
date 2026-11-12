//! CLI execution and service routing.
mod args;
mod catalog;
mod command;
mod graph;
mod input;
mod limits;
mod local;
mod logging;
mod logs;
mod managed;
mod models;
pub(super) mod output;
pub(super) mod parsing;
mod plan;
mod replies;
mod serve_log;

pub(super) use args::Cli;
use comb::Result;
use command::Command;
use serde_json::Value;
use std::path::PathBuf;

pub(super) enum Exit {
    Json(Value),
    Text(String),
    /// Deterministic managed failure already recorded to suppress restarts.
    Suppressed(Value),
    Silent,
}

struct Context {
    comb: PathBuf,
    socket: Option<PathBuf>,
    run: Option<String>,
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .worker_threads(limits::WORKER_THREADS)
        .enable_all()
        .build()?)
}

pub(super) fn run(cli: Cli) -> input::Result<Exit> {
    let mut reaping = hive_setup_support::runner::Reaping::new().map_err(comb::Error::from)?;
    let result = run_owned(cli);
    let cleanup = reaping.restore().map_err(comb::Error::from);
    match result {
        Ok(exit) => {
            cleanup?;
            Ok(exit)
        }
        Err(primary) => Err(primary.with_cleanup(cleanup)),
    }
}

fn run_owned(cli: Cli) -> input::Result<Exit> {
    // Retain caller payloads before inspecting managed state or loading environment.
    let raw = match cli.command {
        args::RawCommand::Setup(options) if options.preparation.apply_prepared.is_some() => {
            args::RawCommand::Setup(input::setup::saved(
                cli.comb.as_deref(),
                cli.socket.as_deref(),
                options,
            )?)
        }
        raw => raw,
    };
    let command = Command::try_from(raw)?;
    let mut context = Context {
        comb: match cli.comb {
            Some(root) => root,
            None if matches!(
                &command,
                Command::Local(
                    command::LocalCommand::Setup(_)
                        | command::LocalCommand::SetupMaintenance(_)
                        | command::LocalCommand::Service(..)
                )
            ) =>
            {
                comb::setup::default_root()?
            }
            None => PathBuf::from("."),
        },
        socket: cli.socket,
        run: cli.run,
    };
    context.prepare_outputs(&command)?;
    let managed = managed::prepare(&context, cli.managed_service.as_deref(), &command)?;
    // No worker has started; initial raw retention precedes environment loading.
    let raw = managed
        .as_ref()
        .map(|spec| hive_log::raw::Owner::admit(&spec.log, &std::io::stderr(), ()))
        .transpose()
        .map_err(comb::Error::from)?
        .flatten();

    match command {
        Command::Local(command) => context.run_local(command, managed, raw),
        Command::Remote(command) => context.run_remote(command, managed, raw),
    }
}

impl Context {
    fn run_local(
        &mut self,
        mut command: command::LocalCommand,
        managed: Option<hive_setup_support::service::Spec>,
        raw: Option<hive_log::raw::Owner>,
    ) -> input::Result<Exit> {
        if let Some(exit) = managed::prepared(managed.as_ref(), self.prepare_local(&mut command))? {
            return Ok(exit);
        }
        let standalone = !matches!(command, command::LocalCommand::Serve { .. });
        let exclusive = input::Exclusive::for_command(&command);
        let input_command = match &command {
            command::LocalCommand::Serve { .. } => Some("serve"),
            command::LocalCommand::Setup(_) => Some("setup"),
            command::LocalCommand::Embed { .. } => Some("embed"),
            _ => None,
        };
        managed::execute(managed, raw, standalone, || command.execute(self)).map_err(|error| {
            let failure = if let Some(name) = input_command {
                input::Failure::remote(error, name, None)
            } else {
                error.into()
            };
            failure.exclusive(exclusive)
        })
    }

    fn run_remote(
        &mut self,
        command: command::RemoteCommand,
        managed: Option<hive_setup_support::service::Spec>,
        raw: Option<hive_log::raw::Owner>,
    ) -> input::Result<Exit> {
        let name = command.name();
        let source = match &command {
            command::RemoteCommand::Put(args) => Some(if args.file.is_some() {
                "--file"
            } else {
                "stdin"
            }),
            _ => None,
        };
        let plan = command.plan(self)?;
        let plan = plan.prepare(|| self.prepare_knowledge(name, true))?;
        managed::execute(managed, raw, true, || Ok(Exit::Json(plan.execute(self)?)))
            .map_err(|error| input::Failure::remote(error, name, source))
    }

    fn prepare_outputs(&self, command: &Command) -> input::Result<()> {
        let Some(path) = &self.socket else {
            return Ok(());
        };
        match command {
            Command::Local(command::LocalCommand::Serve { .. }) => {
                let selected = input::Context::new("serve", "--socket");
                selected.output_name(path)?;
                selected
                    .socket_output(path, comb::service::check_socket_output(&self.comb, path))?;
            }
            Command::Local(command::LocalCommand::Setup(_)) => {
                let selected = input::Context::new("setup", "--socket");
                selected.output_name(path)?;
                selected.socket_output(path, comb::setup::check_socket_output(path))?;
            }
            _ => {}
        }
        Ok(())
    }
    fn prepare_knowledge(&mut self, name: &'static str, socket: bool) -> input::Result<()> {
        input::Context::new(name, "--root").root(&self.comb)?;
        if socket && let Some(path) = &self.socket {
            self.socket = Some(input::Context::new(name, "--socket").socket(&self.comb, path)?);
        }
        Ok(())
    }

    fn prepare_local(&mut self, command: &mut command::LocalCommand) -> input::Result<()> {
        use command::LocalCommand;
        match command {
            LocalCommand::Serve { args, config, plan } => {
                self.prepare_knowledge("serve", true)?;
                *plan = Some(Box::new(
                    comb::provision::plan_server(
                        &self.comb,
                        comb::service::Options {
                            socket: self.socket.clone(),
                            embeddings: config.take(),
                            replace_embeddings: args.replace_embeddings,
                            retry_failed: args.retry_failed,
                            vector_cache_mib: args.vector_cache_mib,
                        },
                    )
                    .map_err(|error| input::Failure::remote(error, "serve", None))?,
                ));
                Ok(())
            }
            LocalCommand::Embed { .. } => self.prepare_knowledge("embed", false),
            LocalCommand::Rebuild => self.prepare_knowledge("sync", false),
            LocalCommand::Mcp { connection } => {
                if let Some(path) = &self.socket {
                    connection.socket =
                        input::Context::new("mcp", "--socket").socket(&self.comb, path)?;
                }
                Ok(())
            }
            LocalCommand::Logs(_) => input::Context::new("logs", "--root").root(&self.comb),
            LocalCommand::Service(..)
            | LocalCommand::Setup(_)
            | LocalCommand::SetupMaintenance(_)
            | LocalCommand::Model(_)
            | LocalCommand::Version { .. }
            | LocalCommand::Init { .. }
            | LocalCommand::Upgrade { .. } => Ok(()),
        }
    }
}
