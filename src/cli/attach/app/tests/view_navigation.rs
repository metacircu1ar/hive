//! View navigation regressions.
use super::*;

#[test]
fn read_only_views_never_write() {
    let mut app = App::new(
        String::from("a"),
        Mode {
            read_only: true,
            ..Mode::default()
        },
    );
    app.update(snapshot("idle", true));
    typed(&mut app, "hello");
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.commands.is_empty() && app.input.is_empty());
    assert!(
        app.screen(true, Rect::new(0, 0, 80, 24))
            .unwrap()
            .input
            .is_none()
    );
}

#[test]
fn scrolling_down_after_the_top_is_reached_moves_at_once() {
    let mut app = app("idle", false);
    app.shown = Shown {
        lines: 100,
        height: 10,
        more: true,
        ..Shown::default()
    };
    for _ in 0..12 {
        press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    }
    // The next frame rendered everything: 100 lines, so the top is at 90.
    app.shown.more = false;
    press(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    assert_eq!(app.scroll, 80);
}

#[test]
fn clicks_focus_the_part_under_them_and_the_wheel_scrolls() {
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    let mut app = app("idle", false);
    app.update(Update::Approval(json!({"id": "q1", "message": "Deploy?"})));
    let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
    terminal
        .draw(|frame| app.draw_into(frame, Rect::new(0, 0, 60, 24), true))
        .unwrap();
    let regions = app.shown.regions;
    let inside = |area: Rect| (area.x + 1, area.y + 1);
    let left = MouseEventKind::Down(MouseButton::Left);
    assert!(mouse_at(&mut app, left, inside(regions.card)));
    assert_eq!(app.focus, Focus::Card);
    assert!(mouse_at(&mut app, left, inside(regions.transcript)));
    assert_eq!(app.focus, Focus::Transcript);
    assert!(mouse_at(&mut app, left, inside(regions.input)));
    assert_eq!(app.focus, Focus::Input);
    // The wheel scrolls three lines a step; scrolling past the top asks for
    // older history, as PgUp does.
    app.shown.more = true;
    assert!(mouse_at(
        &mut app,
        MouseEventKind::ScrollUp,
        inside(regions.transcript)
    ));
    assert_eq!(app.scroll, 3);
    assert!(mouse_at(
        &mut app,
        MouseEventKind::ScrollDown,
        inside(regions.transcript)
    ));
    assert_eq!(app.scroll, 0);
    // Outside the view, and for releases or moves, nothing changes.
    assert!(!mouse_at(&mut app, left, (70, 30)));
    assert!(!mouse_at(
        &mut app,
        MouseEventKind::Moved,
        inside(regions.input)
    ));
    assert!(!mouse_at(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        inside(regions.input)
    ));
    // A click disarms the Ctrl-C confirmation, as a key does.
    app.quit_armed = true;
    assert!(mouse_at(&mut app, left, inside(regions.transcript)));
    assert!(!app.quit_armed);
}

#[test]
fn only_the_active_view_of_a_split_places_the_cursor() {
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    let mut left = app("idle", false);
    let mut right = app("idle", false);
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    let (first, second) = (Rect::new(0, 0, 40, 20), Rect::new(40, 0, 40, 20));
    for active_right in [true, false] {
        terminal
            .draw(|frame| {
                left.draw_into(frame, first, !active_right);
                right.draw_into(frame, second, active_right);
            })
            .unwrap();
        let backend = terminal.backend();
        assert!(backend.cursor_visible());
        let x = backend.cursor_position().x;
        assert_eq!(x >= 40, active_right, "cursor at {x}");
    }
    // No active view: no cursor at all.
    terminal
        .draw(|frame| {
            left.draw_into(frame, first, false);
            right.draw_into(frame, second, false);
        })
        .unwrap();
    assert!(!terminal.backend().cursor_visible());
    // Before the snapshot the view says what it waits for.
    let mut connecting = App::new(String::from("agent-9"), Mode::default());
    connecting.update(Update::Waiting(true));
    terminal
        .draw(|frame| connecting.draw_into(frame, first, true))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let top: String = (0..40)
        .map(|x| buffer[(x, 0)].symbol().to_owned())
        .collect();
    assert!(
        top.starts_with("Connecting to agent-9: waiting for a"),
        "{top}"
    );
}

#[test]
fn capacity_waits_show_and_clear_in_the_status() {
    let mut app = App::new(String::from("a"), Mode::default());
    app.update(Update::Waiting(true));
    assert!(app.waiting() && app.status == WAITING);
    app.update(snapshot("idle", false));
    app.update(Update::Waiting(false));
    assert!(!app.waiting());
    assert!(app.status.starts_with("Enter send"), "{}", app.status);
    // A cleared wait does not replace a newer message.
    app.update(Update::Waiting(true));
    app.update(Update::Problem((String::from("other")).into()));
    app.update(Update::Waiting(false));
    assert_eq!(app.status, "other");
}
