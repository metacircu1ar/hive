//! History loading regressions.
use super::*;

#[test]
fn failed_history_pages_can_be_retried_and_refusals_are_shown() {
    let mut app = app("idle", false);
    app.shown = Shown {
        lines: 30,
        height: 10,
        more: false,
        ..Shown::default()
    };
    press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    assert_eq!(app.commands.len(), 1);
    app.commands.clear();
    app.update(Update::History(Err((String::from(
        "daemon_stopping: stopping",
    ))
    .into())));
    assert!(app.status.contains("older history: daemon_stopping"));
    // The failure released the request, so scrolling up asks again.
    press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    assert_eq!(app.commands.len(), 1);
    app.update(Update::Problem(
        (String::from("invalid_value: /after")).into(),
    ));
    assert_eq!(app.status, "invalid_value: /after");
}

#[test]
fn scrolling_to_the_top_requests_older_history_once() {
    let mut app = app("idle", false);
    app.shown = Shown {
        lines: 30,
        height: 10,
        more: false,
        ..Shown::default()
    };
    press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    let history: Vec<_> = app
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::History { before } => Some(*before),
            Command::Write { .. } | Command::Usage { .. } => None,
        })
        .collect();
    assert_eq!(history, [4]);
    assert_eq!(app.scroll, 20);
}
