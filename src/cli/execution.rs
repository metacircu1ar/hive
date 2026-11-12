//! Execution for this component.
use super::api;
use super::{
    Cli, Command, CommandOutcome, EXIT_ERROR, Error, Result, attach, dispatch, execute, live,
    logging, logs, panes, parsing, read, services, tui, version,
};
use std::process::ExitCode;

pub(super) fn run_parsed(cli: Cli) -> ExitCode {
    let status = match &cli.command {
        Command::Version { json } => version::run(*json),
        Command::Logs(args) => logs::run(&cli, args),
        Command::Read(args) if args.follow => read::run(&cli, args),
        Command::Live(args) if args.follow => live::run(&cli, args),
        Command::Attach(args) => attach::run(&cli, args),
        Command::Panes(args) => panes::run(&cli, args),
        Command::Tui(args) => tui::run(&cli, args),
        Command::Mcp => match logging::run(|| serve_mcp(&cli)) {
            Ok(()) => 0,
            Err(error) => parsing::emit_stderr(parsing::human(&error)),
        },
        _ => run_command(cli),
    };
    crate::process::exit_code(status)
}

fn run_command(cli: Cli) -> i32 {
    let prepared = match dispatch::Prepared::new(cli) {
        Ok(prepared) => prepared,
        Err(failure) => return parsing::emit_file_failure(&failure),
    };
    let logging::Execution { result, spec } = execute(prepared);
    let (value, code, human, suppressed) = match result {
        Ok(CommandOutcome {
            input: Some(input), ..
        }) => return input.emit(),
        Ok(CommandOutcome {
            value,
            success,
            input: None,
        }) => (value, i32::from(!success), String::new(), false),
        Err(ref error @ Error::Input { ref input, .. }) => {
            let value = api::value(api::ErrorResponse {
                error: error.clone(),
            });
            return parsing::emit_remote_input(input, &value, None);
        }
        Err(error) => {
            let suppressed = services::suppress(spec.as_ref(), &error);
            let human = parsing::human(&error);
            (
                api::value(api::ErrorResponse { error }),
                EXIT_ERROR,
                human,
                suppressed,
            )
        }
    };
    parsing::emit_command(&value, code, human, suppressed)
}

pub(super) fn serve_mcp(cli: &Cli) -> Result<()> {
    let root = cli.state_root()?;
    let executor = crate::calls::executor()?;
    let result = executor.block_on(crate::external_mcp::serve(root));
    // Tokio stdio uses blocking workers. An unread stdout must not make
    // Runtime::drop wait forever after EOF or transport overload.
    executor.shutdown_background();
    result
}
