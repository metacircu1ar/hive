//! `hivectl attach`: a terminal chat view of one agent, built on `attach` and `live`.
mod app;
mod card;
mod client;
mod model;
mod picker;
mod usage;
mod view;
mod widget;

pub(super) use app::{END_LINGER, Exit, Mode};
pub(super) use picker::{Kind, List, Lists, MAX_LISTED};
pub(super) use widget::View;

use super::{Cli, Error, Result, api};
use ratatui::crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste},
    execute,
};
use serde_json::Value;
use std::{
    io::IsTerminal,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

// The UI loop polls input this often, so signals and updates are never late.
const TICK: Duration = Duration::from_millis(100);

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct Refusal<'a> {
    #[serde(borrow, deserialize_with = "super::views::object")]
    error: RefusalCode<'a>,
}
#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct RefusalCode<'a> {
    #[serde(borrow, deserialize_with = "super::views::field")]
    code: Option<&'a str>,
}

#[derive(clap::Args)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "These independent CLI switches may be combined; Clap enforces the declared \
            dependencies."
)] // Independent command-line switches.
pub(super) struct Attach {
    #[arg(value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    /// Agent ID to open.
    target: String,
    /// Hide the input and every key that writes.
    #[arg(long)]
    read_only: bool,
    /// Leave once the agent has ended and its final state was shown for --linger seconds.
    #[arg(long)]
    exit_on_end: bool,
    /// Seconds an ended agent stays on screen with --exit-on-end, 0–60 (default 3).
    #[arg(long, requires = "exit_on_end", value_parser = clap::value_parser!(u64).range(0..=60))]
    linger: Option<u64>,
    /// Title the terminal hive:<agent ID> so a pane controller can find it (with --exit-on-end).
    #[arg(long, requires = "exit_on_end")]
    title: bool,
    /// Leave the mouse to the terminal: no clicks or wheel scrolling in the view.
    #[arg(long)]
    no_mouse: bool,
}

/// The terminal title for a view a pane controller opened: only for an agent
/// ID, never for free text.
fn title(enabled: bool, target: &str) -> Option<String> {
    (enabled && uuid::Uuid::parse_str(target).is_ok()).then(|| format!("hive:{target}"))
}

pub(super) fn run(cli: &Cli, args: &Attach) -> i32 {
    let result = if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        cli.state_root().and_then(|root| {
            // Diagnostics wait until the terminal is restored.
            super::logging::run_held(|| view(root, args))
        })
    } else {
        Err(Error::interactive_terminal_required(
            "hivectl attach needs a terminal for stdin and stdout; use hivectl live for scripts.",
        ))
    };
    match result {
        Ok(None) => 0,
        // The daemon refused to attach: report its error like any other command.
        Ok(Some(reply)) => {
            let refusal: Refusal<'_> = super::views::read(&reply);
            let code = refusal.error.code.unwrap_or("internal_error");
            let human = format!("Hive: cannot attach to the agent ({code}).\n");
            super::parsing::emit_failure_record(&reply, human)
        }
        Err(error) => {
            let human = super::parsing::human(&error);
            super::parsing::emit_failure_record(&api::value(api::ErrorResponse { error }), human)
        }
    }
}

/// Run the view; `Some` holds the daemon's refusal of the first agent.
fn view(root: PathBuf, args: &Attach) -> Result<Option<Value>> {
    let signals = Arc::new(AtomicUsize::new(0));
    crate::os::install_cancellation(&signals).map_err(Error::state_io_from)?;
    let mut terminal = ratatui::try_init().map_err(terminal_error)?;
    super::hangup::watch();
    super::mouse::restore_on_panic();
    let pasting = execute!(std::io::stdout(), EnableBracketedPaste).is_ok();
    let clicking =
        !args.no_mouse && execute!(std::io::stdout(), super::mouse::Reporting(true)).is_ok();
    let mut target = args.target.clone();
    let mut previous: Option<String> = None;
    let mut notice = None;
    let result = loop {
        match follow(
            &mut terminal,
            Follow {
                root: &root,
                target: &target,
                args,
                notice: notice.take(),
            },
            &signals,
        ) {
            Ok(app::Exit::Attach(next)) => previous = Some(std::mem::replace(&mut target, next)),
            // A refused `/attach` returns to the agent that was open before.
            Ok(app::Exit::Refused(reply)) => match previous.take() {
                Some(back) => {
                    let code = super::views::error_code(&reply).as_str();
                    notice = Some(format!("cannot attach {target}: {code}"));
                    target = back;
                }
                None => break Ok(Some(reply)),
            },
            Ok(app::Exit::Quit) => break Ok(None),
            Err(error) => break Err(error),
        }
    };
    if clicking {
        // Restoring the terminal below matters more than this mode.
        let _ = execute!(std::io::stdout(), super::mouse::Reporting(false));
    }
    if pasting {
        // Restoring the terminal below matters more than this mode.
        let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    }
    if args.title {
        // A surviving shell must not keep identifying itself as this view.
        let _ = execute!(
            std::io::stdout(),
            ratatui::crossterm::terminal::SetTitle("")
        );
    }
    crate::observability::settle(
        result,
        ratatui::try_restore().map_err(terminal_error),
        "attach_restore",
    )
}

struct Follow<'a> {
    root: &'a std::path::Path,
    target: &'a str,
    args: &'a Attach,
    notice: Option<String>,
}
fn follow(
    terminal: &mut ratatui::DefaultTerminal,
    follow: Follow<'_>,
    signals: &AtomicUsize,
) -> Result<app::Exit> {
    let Follow {
        root,
        target,
        args,
        notice,
    } = follow;
    let mode = app::Mode {
        read_only: args.read_only,
        exit_on_end: args.exit_on_end,
        linger: args.linger.map_or(app::END_LINGER, Duration::from_secs),
        lists: std::sync::Arc::default(),
    };
    let mut shown_title = title(args.title, target);
    if let Some(title) = &shown_title {
        // A missing title only hides the pane from its controller's adoption.
        let _ = execute!(
            std::io::stdout(),
            ratatui::crossterm::terminal::SetTitle(title)
        );
    }
    let mut view = View::open(root, target, mode);
    if let Some(notice) = notice {
        view.set_status(notice);
    }
    // Frames are drawn only after a change, so an idle view costs nothing.
    let mut dirty = true;
    loop {
        dirty |= view.pump();
        // Use the resolved identity too: /attach may receive a relative target
        // or an abbreviated ID rather than the full UUID in the first command.
        let current_title = view.agent_id().and_then(|id| title(args.title, id));
        shown_title = adopt_title(shown_title, current_title);
        if signals.load(Ordering::Relaxed) > 0 {
            return Ok(app::Exit::Quit);
        }
        view.tick(Instant::now());
        if let Some(exit) = view.take_exit() {
            return Ok(exit);
        }
        if dirty {
            terminal
                .draw(|frame| view.draw(frame, frame.area(), true))
                .map_err(terminal_error)?;
            dirty = false;
        }
        if event::poll(TICK).map_err(terminal_error)? {
            dirty |= view.event(&event::read().map_err(terminal_error)?);
            if let Some(text) = view.take_clipboard() {
                // The status line shows the text too; a lost copy is not worth ending the view.
                let _ = copy_to_clipboard(&mut std::io::stdout(), &text);
            }
        }
    }
}

fn adopt_title(shown: Option<String>, current: Option<String>) -> Option<String> {
    if current.is_some()
        && current != shown
        && execute!(
            std::io::stdout(),
            ratatui::crossterm::terminal::SetTitle(current.as_deref().unwrap_or_default())
        )
        .is_ok()
    {
        current
    } else {
        shown
    }
}

/// The OSC 52 sequence that sets the terminal's clipboard. It travels with the
/// output, so it reaches the local terminal over SSH and through zellij; tmux
/// passes it on with `set-clipboard on`.
pub(super) fn osc52(text: &str) -> String {
    use base64::Engine as _;
    format!(
        "\x1b]52;c;{}\x07",
        base64::engine::general_purpose::STANDARD.encode(text)
    )
}

pub(super) fn copy_to_clipboard(out: &mut impl std::io::Write, text: &str) -> std::io::Result<()> {
    out.write_all(osc52(text).as_bytes())?;
    out.flush()
}

pub(super) fn terminal_error(cause: std::io::Error) -> Error {
    Error::state_io("Cannot drive the terminal.").with_source(cause)
}

#[cfg(test)]
mod tests;
