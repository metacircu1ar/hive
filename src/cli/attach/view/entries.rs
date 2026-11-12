//! Render one transcript entry without changing wrapping or tool-tail policy.
use super::{Color, Entry, Line, Modifier, Span, Style, TOOL_TAIL_LINES, markdown, wrap};

pub(super) fn entry_lines(entry: &Entry, width: usize) -> Vec<Line<'static>> {
    let dim = Style::new().add_modifier(Modifier::DIM);
    match entry {
        Entry::Input { sender, body, mode } => input_lines(sender, body, mode.as_deref(), width),
        Entry::Assistant { text, live, .. } => {
            let shown = if live.len() > text.len() { live } else { text };
            markdown(shown, width)
        }
        Entry::Reasoning { text, .. } => {
            let style = dim.add_modifier(Modifier::ITALIC);
            wrap(&[(format!("· {text}"), style)], width)
        }
        Entry::Tool {
            tool,
            summary,
            status,
            output,
            running,
            ..
        } => ToolLines {
            tool,
            summary,
            status: status.as_deref(),
            output,
            running: *running,
        }
        .render(width, dim),

        Entry::TurnStarted { model, effort } => {
            vec![Line::from(Span::styled(
                format!("── turn: {model} {effort}"),
                dim,
            ))]
        }
        Entry::TurnFinished { status } => {
            vec![Line::from(Span::styled(format!("── {status}"), dim))]
        }
        Entry::Approval { id, title, open } => {
            let state = if *open { "waiting" } else { "closed" };
            let title = title.clone().unwrap_or_else(|| id.clone());
            wrap(
                &[(
                    format!("? {title} ({state})"),
                    Style::new().fg(Color::Yellow),
                )],
                width,
            )
        }
        Entry::Notice(text) => wrap(&[(format!("• {text}"), dim)], width),
    }
}
fn input_lines(sender: &str, body: &str, mode: Option<&str>, width: usize) -> Vec<Line<'static>> {
    let label = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let mode = mode
        .filter(|mode| *mode != "next-turn")
        .map_or_else(String::new, |mode| format!(" [{mode}]"));
    wrap(
        &[
            (format!("{sender}{mode}: "), label),
            (body.to_owned(), Style::new()),
        ],
        width,
    )
}
struct ToolLines<'a> {
    tool: &'a str,
    summary: &'a str,
    status: Option<&'a str>,
    output: &'a str,
    running: bool,
}
impl ToolLines<'_> {
    fn render(&self, width: usize, dim: Style) -> Vec<Line<'static>> {
        let Self {
            tool,
            summary,
            status,
            output,
            running,
        } = *self;
        let state = if running {
            String::from("running")
        } else {
            status.unwrap_or("done").to_owned()
        };
        let colour = if running { Color::Yellow } else { Color::Green };
        let segments = [
            (format!("⚙ {tool} "), Style::new().fg(colour)),
            (summary.to_owned(), Style::new()),
            (format!("  {state}"), dim),
        ];
        let tail: Vec<&str> = output.lines().rev().take(TOOL_TAIL_LINES).collect();
        wrap(&segments, width)
            .into_iter()
            .chain(
                tail.into_iter()
                    .rev()
                    .flat_map(|line| wrap(&[(format!("  {line}"), dim)], width)),
            )
            .collect()
    }
}
