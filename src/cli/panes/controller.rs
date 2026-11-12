//! Observe panes, reconcile placement and handle one terminal controller.
use super::{
    Action, AtomicUsize, Controller, Event, Failure, Feed, Instant, KeyCode, KeyEventKind,
    KeyModifiers, Limits, Line, Modifier, Ordering, PaneCommand, Paragraph, Receiver, Result, Span,
    Style, TICK, Value, event, plan, rows, terminal_error,
};
use crate::cli::views;

enum KeyAction {
    Quit,
    Reopen,
    Other,
}

impl Controller<'_> {
    pub(super) fn run(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
        feed: &Receiver<Feed>,
        signals: &AtomicUsize,
    ) -> Result<Option<Value>> {
        let mut force = true;
        let mut last: Option<Instant> = None;
        let mut dirty = true;
        loop {
            while let Ok(update) = feed.try_recv() {
                match update {
                    Feed::Tree(reply) => {
                        self.rows = rows(&reply);
                        self.truncated = views::read::<views::TreeMetadata>(&reply).truncated;
                        (self.connected, self.waiting) = (true, false);
                        force = true;
                    }
                    Feed::Waiting => self.waiting = true,
                    Feed::Disconnected => self.connected = false,
                    Feed::Refused(reply) => return Ok(Some(reply)),
                }
                dirty = true;
            }
            if signals.load(Ordering::Relaxed) > 0 {
                return Ok(None);
            }
            let now = Instant::now();
            let due = self.pass_due(force, last, now);
            if due && self.connected {
                force = self.pass(now)?;
                last = Some(now);
                dirty = true;
            }
            if dirty {
                self.render(terminal)?;
                dirty = false;
            }
            if event::poll(TICK).map_err(terminal_error)?
                && let Event::Key(key) = event::read().map_err(terminal_error)?
                && key.kind != KeyEventKind::Release
            {
                dirty = true;
                match self.key_action(key) {
                    KeyAction::Quit => return Ok(None),
                    KeyAction::Reopen => force = true,
                    KeyAction::Other => {}
                }
            }
        }
    }

    fn pass_due(&self, force: bool, last: Option<Instant>, now: Instant) -> bool {
        // A deadline is due once: the pass after it settles the state that
        // produced it, even if a retry still waits for room.
        let deadline = self
            .history
            .deadline()
            .is_some_and(|deadline| now >= deadline && last.is_none_or(|last| deadline > last));
        // Panes are listed regularly too, to notice panes the user closed.
        let every = self.backend.list_every();
        force || deadline || last.is_none_or(|last| now.duration_since(last) >= every)
    }

    fn render(&self, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new(self.lines(frame.area().height)),
                    frame.area(),
                );
            })
            .map_err(terminal_error)?;
        Ok(())
    }

    fn key_action(&mut self, key: ratatui::crossterm::event::KeyEvent) -> KeyAction {
        match key.code {
            KeyCode::Char('q') => KeyAction::Quit,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => KeyAction::Quit,
            KeyCode::Char('o') => {
                self.history.reopen();
                KeyAction::Reopen
            }
            _ => KeyAction::Other,
        }
    }

    /// List panes, reconcile and run the actions; `true` asks for another
    /// pass. A backend that cannot work at all ends the controller.
    fn pass(&mut self, now: Instant) -> Result<bool> {
        let panes = match self.backend.panes() {
            Ok(panes) => panes,
            Err(Failure::Retry(problem)) => {
                self.message = problem;
                return Ok(false);
            }
            Err(Failure::Fatal(error)) => return Err(error),
        };
        if let Some(notice) = self.backend.take_notice() {
            self.message = notice;
        }
        let (per_tab, stacks) = self.backend.layout();
        let limits = Limits {
            max_panes: usize::from(self.args.limit),
            per_tab,
            stacks,
        };
        let pass = plan::reconcile(&self.rows, &panes, &mut self.history, &limits, now);
        for action in &pass.actions {
            self.act(action, now)?;
        }
        self.status = plan::status_list(&self.rows, &panes, &self.history);
        Ok(pass.again)
    }

    fn act(&mut self, action: &Action, now: Instant) -> Result<()> {
        let agent = match action {
            Action::OpenTab { agent, .. } | Action::OpenPane { agent, .. } => agent,
        };
        let mut args = vec![
            String::from("--state-dir"),
            self.root.to_string_lossy().into_owned(),
            String::from("attach"),
            agent.clone(),
            String::from("--exit-on-end"),
            String::from("--linger"),
            self.args.linger.to_string(),
        ];
        args.extend(
            self.backend
                .attach_flags()
                .iter()
                .map(|flag| (*flag).to_owned()),
        );
        if self.args.no_mouse {
            args.push(String::from("--no-mouse"));
        }
        let command = PaneCommand {
            agent: agent.clone(),
            program: self.program.clone(),
            args,
        };
        let result = match action {
            Action::OpenTab { name, .. } => self.backend.open_tab(name, &command).map(Some),
            Action::OpenPane { tab, stacked, .. } => self
                .backend
                .open_pane(*tab, &command, *stacked)
                .map(|()| None),
        };
        match result {
            Ok(tab) => {
                self.history.opened(action, tab, now);
                if let Some(notice) = self.backend.take_notice() {
                    self.message = notice;
                }
            }
            Err(Failure::Retry(problem)) => {
                self.history.failed(agent, now);
                self.message = problem;
            }
            Err(Failure::Fatal(error)) => return Err(error),
        }
        Ok(())
    }

    fn lines(&self, height: u16) -> Vec<Line<'static>> {
        let dim = Style::new().add_modifier(Modifier::DIM);
        let open = self
            .status
            .iter()
            .filter(|status| status.pane == "open")
            .count();
        let mut header = format!(
            "Hive panes: {} agents, {open} open (limit {})",
            self.rows.len(),
            self.args.limit
        );
        if self.truncated {
            header.push_str(", tree truncated at 1,000 agents");
        }
        if !self.connected {
            header.push_str(", disconnected; retrying");
        }
        if self.waiting {
            header.push_str(", waiting for a viewer slot");
        }
        let mut lines = vec![
            Line::from(Span::styled(
                header,
                Style::new().add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "o reopen closed panes · q quit (panes stay open)",
                dim,
            )),
        ];
        if !self.message.is_empty() {
            lines.push(Line::from(self.message.clone()));
        }
        let room = usize::from(height).saturating_sub(lines.len() + 1);
        for status in self.status.iter().take(room) {
            let label = status.name.clone().unwrap_or_else(|| {
                format!(
                    "{} {}",
                    status.id.get(..8).unwrap_or(&status.id),
                    status.role.as_deref().unwrap_or("agent")
                )
            });
            lines.push(Line::from(vec![
                Span::raw(format!("{}{label} ", "  ".repeat(status.depth.min(8)))),
                Span::styled(format!("{} · {}", status.state, status.pane), dim),
            ]));
        }
        if self.status.len() > room {
            lines.push(Line::from(Span::styled(
                format!("… {} more", self.status.len() - room),
                dim,
            )));
        }
        lines
    }
}
