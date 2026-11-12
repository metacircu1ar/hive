//! `hivectl panes`: a zellij, tmux or Ghostty pane per agent, opened and closed
//! with the agents.
//!
//! The controller follows the agent tree and keeps one `hivectl attach` pane per
//! live agent in scope. Panes close themselves when their agent ends; leaving
//! the controller never closes panes or stops agents.
mod controller;
mod startup;
use startup::{ghostty, tmux, tmux_owns_terminal, tmux_pane, zellij};

mod backend;
mod ghostty;
mod plan;
mod tmux;

use super::tree_feed::{self, Feed};
use super::{Cli, Error, Result, api};
use backend::{Backend, Failure, PaneCommand, Zellij};
use plan::{Action, History, Limits, Row};
use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use serde_json::Value;
use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::Receiver,
    },
    time::{Duration, Instant},
};

// The loop polls keys this often, so passes and signals are never late.
const TICK: Duration = Duration::from_millis(100);

#[derive(clap::Args)]
pub(super) struct Panes {
    /// Only this agent and its descendants; by default every live agent.
    #[arg(long, value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    root: Option<String>,
    /// Most agent panes open at once, 1–32 (default 16); each holds one daemon long-poll.
    #[arg(long = "max-panes",
         default_value_t = 16,
         value_parser = clap::value_parser!(u16).range(1..=32))]
    limit: u16,
    /// Seconds an ended agent's pane stays open, 0–60 (default 3).
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u64).range(0..=60))]
    linger: u64,
    #[doc = " Pane backend; by default zellij or tmux, whichever runs this terminal, else \
        Ghostty on macOS."]
    #[arg(long, value_enum)]
    backend: Option<Kind>,
    /// Open views without mouse support, leaving clicks and the wheel to the terminal.
    #[arg(long)]
    no_mouse: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Kind {
    Zellij,
    Tmux,
    Ghostty,
}

enum Multiplexers {
    None,
    Zellij,
    Tmux,
    Both,
}

/// The multiplexer running this terminal. When one runs inside the other, both
/// pass their variables on; tmux wins only if it owns this terminal, which
/// `tmux_owns` checks (any failure means it does not).
fn multiplexer(detected: Multiplexers, tmux_owns: impl FnOnce() -> bool) -> Option<Kind> {
    match detected {
        Multiplexers::Both if tmux_owns() => Some(Kind::Tmux),
        Multiplexers::Both | Multiplexers::Zellij => Some(Kind::Zellij),
        Multiplexers::Tmux => Some(Kind::Tmux),
        Multiplexers::None => None,
    }
}

/// The backend for this terminal: explicit, or the multiplexer running it, or
/// Ghostty when its terminal runs the controller on macOS.
fn kind(explicit: Option<Kind>, multiplexer: Option<Kind>, ghostty: bool) -> Result<Kind> {
    match (explicit, multiplexer) {
        (Some(Kind::Ghostty), _) if !cfg!(target_os = "macos") => {
            Err(Error::pane_backend_unavailable(
                "Ghostty panes need macOS; on Linux, run hivectl panes inside zellij or tmux.",
            ))
        }
        (Some(kind), _) | (None, Some(kind)) => Ok(kind),
        (None, None) if ghostty && cfg!(target_os = "macos") => Ok(Kind::Ghostty),
        (None, None) => Err(Error::pane_backend_unavailable(
            "hivectl panes runs inside zellij or tmux, or in a Ghostty terminal on \
                macOS; elsewhere, use hivectl tui.",
        )),
    }
}

pub(super) fn run(cli: &Cli, args: &Panes) -> i32 {
    match prepare(cli, args).and_then(|setup| super::logging::run_held(|| control(setup, args))) {
        Ok(None) => 0,
        Ok(Some(reply)) => {
            let code = super::views::error_code(&reply).as_str();
            let human = format!("Hive: cannot follow the agent tree ({code}).\n");
            super::parsing::emit_failure_record(&reply, human)
        }
        Err(error) => {
            let human = super::parsing::human(&error);
            super::parsing::emit_failure_record(&api::value(api::ErrorResponse { error }), human)
        }
    }
}

struct Setup {
    root: PathBuf,
    program: PathBuf,
    backend: Box<dyn Backend>,
    /// Held for the controller's lifetime: one controller per zellij session,
    /// or per state root for Ghostty.
    _lock: std::fs::File,
}

/// Check the terminal, the backend, the daemon's state and the lock.
fn prepare(cli: &Cli, args: &Panes) -> Result<Setup> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(Error::interactive_terminal_required(
            "hivectl panes needs a terminal for stdin and stdout; run it in a zellij or tmux \
                pane.",
        ));
    }
    let tmux_pane = tmux_pane();
    let detected = match (std::env::var_os("ZELLIJ").is_some(), tmux_pane.is_some()) {
        (true, true) => Multiplexers::Both,
        (true, false) => Multiplexers::Zellij,
        (false, true) => Multiplexers::Tmux,
        (false, false) => Multiplexers::None,
    };
    let multiplexer = multiplexer(detected, || {
        tmux_pane.as_deref().is_some_and(tmux_owns_terminal)
    });
    let kind = kind(
        args.backend,
        multiplexer,
        std::env::var("TERM_PROGRAM").is_ok_and(|program| program == "ghostty"),
    )?;
    let root = cli
        .state_root()?
        .canonicalize()
        .map_err(Error::state_io_from)?;
    let runtime = root.join("runtime");
    if !runtime.is_dir() {
        return Err(Error::daemon_unavailable(
            "Start the Hive daemon for this state directory first.",
        ));
    }
    let (backend, lock_name): (Box<dyn Backend>, String) = match kind {
        Kind::Zellij => zellij()?,
        Kind::Tmux => tmux(&root)?,
        Kind::Ghostty => (Box::new(ghostty()?), String::from("ghostty")),
    };
    let lock = lock(&runtime, &lock_name)?;
    let program = std::env::current_exe()
        .and_then(|program| program.canonicalize())
        .map_err(Error::state_io_from)?;
    Ok(Setup {
        root,
        program,
        backend,
        _lock: lock,
    })
}

/// An advisory lock, released when the process dies: per zellij session (the
/// SHA-256 of its name), or `ghostty` for one Ghostty controller per state root.
fn lock(runtime: &Path, name: &str) -> Result<std::fs::File> {
    let path = runtime.join(format!("panes-{name}.lock"));
    let file = crate::os::private_file(&path, true).map_err(Error::state_io_from)?;
    match crate::os::exclusive_lock(&file, true) {
        Ok(()) => {}
        Err(cause) if cause.kind() == std::io::ErrorKind::WouldBlock => {
            return Err(Error::pane_controller_running(
                "Another hivectl panes already controls these panes.",
            ));
        }
        Err(cause) => return Err(Error::state_io_from(cause)),
    }
    Ok(file)
}

/// Live agents from a tree reply, in its order.
fn rows(reply: &Value) -> Vec<Row> {
    let tree: super::views::Tree<'_> = super::views::read(reply);
    tree.agents
        .into_iter()
        .filter(|row| !row.ended)
        .filter_map(|row| {
            Some(Row {
                id: row.id?.to_owned(),
                parent: row.parent_id.map(str::to_owned),
                role: row.role.map(str::to_owned),
                name: row.name.map(str::to_owned),
                state: row.state.unwrap_or_default().to_owned(),
            })
        })
        .collect()
}

struct Controller<'a> {
    root: PathBuf,
    program: PathBuf,
    args: &'a Panes,
    backend: Box<dyn Backend>,
    history: History,
    rows: Vec<Row>,
    status: Vec<plan::Status>,
    connected: bool,
    truncated: bool,
    /// The daemon has no room for the tree's long-poll.
    waiting: bool,
    message: String,
}

fn control(setup: Setup, args: &Panes) -> Result<Option<Value>> {
    let signals = Arc::new(AtomicUsize::new(0));
    crate::os::install_cancellation(&signals).map_err(Error::state_io_from)?;
    let (sender, feed) = std::sync::mpsc::sync_channel(4);
    let root = setup.root.clone();
    let scope = args.root.clone();
    std::thread::spawn(move || tree_feed::follow(root, scope, sender));
    let mut terminal = ratatui::try_init().map_err(terminal_error)?;
    super::hangup::watch();
    let Setup {
        root,
        program,
        backend,
        _lock,
    } = setup;
    let mut controller = Controller {
        root,
        program,
        args,
        backend,
        history: History::default(),
        rows: Vec::new(),
        status: Vec::new(),
        connected: false,
        truncated: false,
        waiting: false,
        message: String::new(),
    };
    match controller.backend.announce() {
        Ok(()) => {}
        Err(Failure::Retry(problem)) => controller.message = problem,
        Err(Failure::Fatal(error)) => {
            return crate::observability::settle(
                Err(error),
                ratatui::try_restore().map_err(terminal_error),
                "panes_restore",
            );
        }
    }
    let result = controller.run(&mut terminal, &feed, &signals);
    crate::observability::settle(
        result,
        ratatui::try_restore().map_err(terminal_error),
        "panes_restore",
    )
}

fn terminal_error(cause: std::io::Error) -> Error {
    Error::state_io("Cannot drive the terminal.").with_source(cause)
}

#[cfg(test)]
mod tests;

const GHOSTTY_READY_TIMEOUT: std::time::Duration = Duration::from_secs(1);

const GHOSTTY_READY_POLL: std::time::Duration = Duration::from_millis(100);
