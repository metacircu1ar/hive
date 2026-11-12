//! Render for the Console view.
use super::{
    Action, Areas, Block, Clear, Color, Console, Constraint, Focus, Frame, Line, Modifier, Overlay,
    Paragraph, Rect, Span, Style, centered, draw_spawn, layout, pointer, sidebar,
};
use std::fmt::Write as _;

impl Console<'_> {
    pub(super) fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let width = (area.width / 4).clamp(20, 36).min(area.width / 2);
        let [side, main] =
            Areas::horizontal([Constraint::Length(width), Constraint::Min(10)]).areas(area);
        let [bar, body, status, buttons] = Areas::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(main);
        let mut hits = pointer::Hits {
            sidebar: side,
            rows: self.draw_sidebar(frame, side),
            bar,
            ..pointer::Hits::default()
        };
        let (tab_line, tabs) = self.tab_line();
        hits.tabs = tabs
            .into_iter()
            .map(|(start, end, index)| (bar.x + start, bar.x + end, index))
            .collect();
        frame.render_widget(Paragraph::new(tab_line), bar);
        hits.views = self.draw_views(frame, body);
        frame.render_widget(Paragraph::new(self.status_line()), status);
        let (button_line, ranges) = self.button_bar();
        frame.render_widget(Paragraph::new(button_line), buttons);
        hits.button_row = buttons;
        hits.buttons = ranges
            .into_iter()
            .map(|(start, end, action)| (buttons.x + start, buttons.x + end, action))
            .collect();
        match &self.overlay {
            Overlay::None => {}
            Overlay::Attention => {
                (hits.overlay, hits.entries) = self.draw_attention(frame, body);
            }
            Overlay::Spawn(dialog) => {
                (hits.overlay, hits.fields) = draw_spawn(frame, body, dialog);
            }
        }
        self.hits = hits;
    }

    fn draw_views(&mut self, frame: &mut Frame<'_>, body: Rect) -> Vec<(String, Rect)> {
        let views_focused = self.focus == Focus::View && matches!(self.overlay, Overlay::None);
        let areas = self.layout.areas(body);
        if areas.is_empty() {
            let prefix = self.prefix();
            frame.render_widget(
                Paragraph::new(format!(
                    "No agent view is open. Pick an agent in the sidebar and press Enter, or \
                        {prefix} c to spawn one."
                ))
                .style(Style::new().add_modifier(Modifier::DIM))
                .wrap(ratatui::widgets::Wrap { trim: true }),
                body,
            );
        }
        areas
            .into_iter()
            .map(|(agent, area, active)| {
                let title = self.title(&agent);
                let style = if active && views_focused {
                    Style::new().fg(Color::Yellow)
                } else {
                    Style::new().add_modifier(Modifier::DIM)
                };
                let block = Block::bordered().title(title).border_style(style);
                let inner = block.inner(area);
                frame.render_widget(block, area);
                if let Some(view) = self.views.get_mut(&agent) {
                    view.draw(frame, inner, active && views_focused);
                }
                (agent, area)
            })
            .collect()
    }

    pub(super) fn title(&self, agent: &str) -> String {
        match self.sidebar.agent(agent) {
            Some(row) => format!(" {} ", row.display()),
            None => format!(" {} ", sidebar::short(agent)),
        }
    }

    /// The tab bar, and each label's columns (start, end) and tab index.
    pub(super) fn tab_line(&self) -> (Line<'static>, Vec<(u16, u16, usize)>) {
        crate::cli::labels::row(self.layout.tabs.iter().enumerate().map(|(index, tab)| {
            let names: Vec<&str> = tab
                .views
                .iter()
                .map(|agent| match self.sidebar.agent(agent) {
                    Some(row) => row.tab_label(),
                    None => sidebar::short(agent),
                })
                .collect();
            let style = if index == self.layout.current {
                Style::new().add_modifier(Modifier::REVERSED)
            } else {
                Style::new().add_modifier(Modifier::DIM)
            };
            let label = format!(" {}:{} ", index + 1, names.join("|"));
            (index, Span::styled(label, style))
        }))
    }

    /// The clickable buttons under the status line, each one a prefix
    /// action, and their columns (start, end).
    pub(super) fn button_bar(&self) -> (Line<'static>, Vec<(u16, u16, Action)>) {
        let buttons = [
            (" Sidebar ", Action::Sidebar),
            (" Split ", Action::Split(layout::Split::Columns)),
            (" Close ", Action::Close),
            (" Questions ", Action::Attention),
            (" Spawn ", Action::Spawn),
            (" Quit ", Action::Quit),
        ];
        let (mut line, ranges) =
            crate::cli::labels::row(buttons.into_iter().map(|(label, action)| {
                (
                    action,
                    Span::styled(label, Style::new().add_modifier(Modifier::REVERSED)),
                )
            }));
        // The pending prefix lists every key; this only says how to get there.
        line.spans.push(Span::styled(
            format!("{}: more keys", self.prefix()),
            Style::new().add_modifier(Modifier::DIM),
        ));
        (line, ranges)
    }

    pub(super) fn status_line(&self) -> Line<'static> {
        let dim = Style::new().add_modifier(Modifier::DIM);
        if self.router.pending() {
            return Line::from(Span::styled(
                format!(
                    "{} … s sidebar  t tab  v|h split  w close  1-9 n p o tabs  a attention  c \
                        spawn  q quit",
                    self.prefix()
                ),
                Style::new().fg(Color::Yellow),
            ));
        }
        let needing = self.attention.entries.len();
        let attention = (needing > 0).then(|| {
            Span::styled(
                format!("! {needing} need you "),
                Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )
        });
        let waiting =
            self.views.values().filter(|view| view.waiting()).count() + usize::from(self.waiting);
        let waiting = (waiting > 0).then(|| {
            let sidebar = if self.waiting {
                " (sidebar included)"
            } else {
                ""
            };
            Span::styled(
                format!("{waiting} waiting for a viewer slot{sidebar} "),
                Style::new().fg(Color::Magenta),
            )
        });
        let disconnected = (!self.connected)
            .then(|| Span::styled("disconnected, retrying ", Style::new().fg(Color::Red)));
        Line::from(
            [
                attention,
                waiting,
                disconnected,
                Some(Span::styled(self.status.clone(), dim)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
        )
    }

    /// Draw the sidebar; returns each agent row drawn, with the column of its
    /// collapse marker when it has children.
    pub(super) fn draw_sidebar(&self, frame: &mut Frame<'_>, area: Rect) -> Vec<pointer::Row> {
        let focused = self.focus == Focus::Sidebar && matches!(self.overlay, Overlay::None);
        let mut title = format!(" agents {} ", self.sidebar.agents().len());
        if self.sidebar.truncated {
            title.push_str("(truncated) ");
        }
        let style = if focused {
            Style::new().fg(Color::Yellow)
        } else {
            Style::new().add_modifier(Modifier::DIM)
        };
        let block = Block::bordered().title(title).border_style(style);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let items = self.sidebar.items();
        let selected = self.sidebar.selection(&items);
        let filter = (!self.sidebar.filter.is_empty()).then(|| {
            Line::from(Span::styled(
                format!("/{}", self.sidebar.filter),
                Style::new().fg(Color::Cyan),
            ))
        });
        let prefix = usize::from(filter.is_some());
        let height = usize::from(inner.height).saturating_sub(prefix).max(1);
        let skip = selected.map_or(0, |selected| (selected + 1).saturating_sub(height));
        let (lines, rows): (Vec<_>, Vec<_>) = items
            .iter()
            .enumerate()
            .skip(skip)
            .take(height)
            .filter_map(|(index, item)| Some((index, item, self.sidebar.agent(&item.id)?)))
            .enumerate()
            .map(|(visible, (index, item, agent))| {
                let selection = (Some(index) == selected).then_some(if focused {
                    Modifier::REVERSED
                } else {
                    Modifier::BOLD
                });
                let (line, marker_offset) = sidebar_line(item, agent, selection);
                let row = sidebar_row(item, inner, prefix + visible, marker_offset);
                (line, row)
            })
            .unzip();
        let mut lines: Vec<_> = filter.into_iter().chain(lines).collect();
        if items.is_empty() {
            let text = if self.connected {
                "no live agents"
            } else {
                "connecting…"
            };
            lines.push(Line::from(Span::styled(
                text,
                Style::new().add_modifier(Modifier::DIM),
            )));
        }
        frame.render_widget(Paragraph::new(lines), inner);
        rows
    }

    /// Draw the questions list; returns its area and each entry's row.
    pub(super) fn draw_attention(
        &self,
        frame: &mut Frame<'_>,
        body: Rect,
    ) -> (Rect, Vec<(u16, usize)>) {
        let entries = &self.attention.entries;
        let height = u16::try_from(entries.len().max(1) + 4).unwrap_or(u16::MAX);
        let area = centered(body, 64, height);
        frame.render_widget(Clear, area);
        let block = Block::bordered()
            .title(format!(" needs you ({}) ", entries.len()))
            .border_style(Style::new().fg(Color::Yellow));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let (mut lines, rows): (Vec<_>, Vec<_>) = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let row = (
                    inner
                        .y
                        .saturating_add(u16::try_from(index).unwrap_or(u16::MAX)),
                    index,
                );
                let label = self.sidebar.agent(&entry.agent).map_or_else(
                    || format!("agent {}", sidebar::short(&entry.agent)),
                    sidebar::Agent::display,
                );
                let mut style = Style::new();
                if index == self.attention.selected {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                (Line::from(Span::styled(format!("! {label}"), style)), row)
            })
            .unzip();
        if entries.is_empty() {
            lines.push(Line::from("Nothing is waiting for you."));
        }
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Enter opens the agent at its question · Esc closes",
            Style::new().add_modifier(Modifier::DIM),
        )));
        frame.render_widget(Paragraph::new(lines), inner);
        (area, rows)
    }
}

fn sidebar_row(
    item: &sidebar::Item,
    inner: Rect,
    row: usize,
    marker_offset: usize,
) -> pointer::Row {
    pointer::Row {
        y: inner
            .y
            .saturating_add(u16::try_from(row).unwrap_or(u16::MAX)),
        agent: item.id.clone(),
        marker: (item.children || item.hidden.is_some())
            .then(|| {
                inner
                    .x
                    .saturating_add(u16::try_from(marker_offset).unwrap_or(u16::MAX))
            })
            .filter(|column| *column < inner.right()),
    }
}

fn sidebar_line(
    item: &sidebar::Item,
    agent: &sidebar::Agent,
    selection: Option<Modifier>,
) -> (Line<'static>, usize) {
    let marker = match (item.hidden, item.children) {
        (Some(_), _) => "▸",
        (None, true) => "▾",
        (None, false) => " ",
    };
    let mut text = format!(
        "{}{marker}{} {}",
        "  ".repeat(item.depth),
        item.badge.symbol(),
        agent.display()
    );
    if let Some(hidden) = item.hidden {
        // Writing to a String cannot fail.
        let _ = write!(text, " +{hidden}");
    }
    let style = selection.map_or(item.badge.style(), |modifier| {
        item.badge.style().add_modifier(modifier)
    });
    let body_width = Span::raw(text.as_str()).width();
    let line = agent.sidebar_line(text, style);
    let marker_offset = line.width().saturating_sub(body_width) + item.depth * 2;
    (line, marker_offset)
}
