//! Human log inspection opens no knowledge database and creates no root.
use super::{Context, Exit};
use comb::Result;
use hive_log::reader::{EventName, Filter, Reader, View};
use std::io;

#[derive(clap::Args)]
pub(super) struct Logs {
    /// Latest matching records (1..2000), inspecting at most 8 MiB.
    #[arg(long, default_value_t = 100, value_parser = hive_input::clap::parsers::count(1..=2000))]
    tail: usize,
    /// Minimum severity: warn includes warnings and errors.
    #[arg(long, value_parser = ["error", "warn", "info", "debug", "trace"])]
    level: Option<String>,
    /// Match an exact static event name, such as comb.ready.
    #[arg(long, value_parser = event_name)]
    event: Option<EventName>,
    /// Print new complete records until Ctrl-C (humans only).
    #[arg(long)]
    follow: bool,
}
fn event_name(text: &str) -> std::result::Result<EventName, EventNameError> {
    EventName::parse(text).map_err(|_| EventNameError)
}
#[derive(Debug)]
struct EventNameError;
impl std::fmt::Display for EventNameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Expected 1..128 ASCII letters, digits, _, ., : or -")
    }
}
impl std::error::Error for EventNameError {}
pub(super) fn run(context: &Context, args: Logs) -> Result<Exit> {
    let result = view(context, args);
    match result {
        Ok(()) => Ok(Exit::Silent),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(Exit::Silent),
        Err(error) => Err(error.into()),
    }
}
fn view(context: &Context, args: Logs) -> io::Result<()> {
    // Location admission checks existing ancestors without opening the store.
    let root = hive_setup_support::files::location(&context.comb)?;
    let reader = Reader::open(&root.join(".comb/logs/comb.jsonl"))?;
    let level = args
        .level
        .map(|text| text.parse::<tracing::Level>())
        .transpose()
        .map_err(io::Error::other)?;
    reader.view(
        &View {
            tail: args.tail,
            filter: Filter {
                level,
                event: args.event.map(EventName::into_string),
                agent_id: None,
            },
            follow: args.follow,
        },
        "Comb",
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    )
}
