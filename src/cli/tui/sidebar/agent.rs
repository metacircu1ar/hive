//! Agent for this component.
use super::{Agent, Badge, Color, Modifier, Style, short};
use super::{AgentState, TaskOutcome};

impl Agent {
    pub(in crate::cli::tui) fn badge(&self) -> Badge {
        if self.approval.is_some() && !self.ended {
            return Badge::NeedsYou;
        }
        if self.ended || self.state.known() == Some(AgentState::Failed) {
            return match self.outcome.known() {
                Some(TaskOutcome::Completed) => Badge::Done,
                Some(TaskOutcome::Cancelled) => Badge::Cancelled,
                _ if self.ended && self.state.known() != Some(AgentState::Failed) => Badge::Ended,
                _ => Badge::Failed,
            };
        }
        if self.waiting.as_deref() == Some("quota") {
            return Badge::Quota;
        }
        match self.state.known() {
            Some(AgentState::Running | AgentState::Starting | AgentState::Stopping) => {
                Badge::Working
            }
            _ => Badge::Idle,
        }
    }

    /// The first eight characters of the agent ID.
    pub(in crate::cli::tui) fn short(&self) -> &str {
        short(&self.id)
    }

    pub(in crate::cli::tui) fn label(&self) -> &str {
        self.role.as_deref().unwrap_or("agent")
    }

    /// How rows, titles and the attention list name the agent: its name, or
    /// its role (or "agent") and short ID.
    pub(in crate::cli::tui) fn display(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("{} {}", self.label(), self.short()))
    }

    /// A tab names its agent by name, or by role.
    pub(in crate::cli::tui) fn tab_label(&self) -> &str {
        self.name.as_deref().unwrap_or_else(|| self.label())
    }

    pub(in crate::cli::tui) fn sidebar_line(
        &self,
        text: String,
        style: Style,
    ) -> ratatui::text::Line<'static> {
        use ratatui::text::{Line, Span};
        let badge = (self.sandbox == "full-access").then(|| {
            Span::styled(
                "[full-access] ",
                Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
            )
        });
        Line::from(
            badge
                .into_iter()
                .chain(self.one_shot.then(|| Span::styled("[one-shot] ", style)))
                .chain(std::iter::once(Span::styled(text, style)))
                .collect::<Vec<_>>(),
        )
    }
}
