//! View termination regressions.
use super::*;

#[test]
fn exit_on_end_leaves_after_the_linger() {
    let mut app = App::new(
        String::from("a"),
        Mode {
            exit_on_end: true,
            ..Mode::default()
        },
    );
    let mut ended = snapshot("idle", false);
    if let Update::Snapshot(reply) = &mut ended {
        reply["agent"]["ended_at"] = json!(10);
    }
    app.update(ended);
    let started = Instant::now();
    app.tick(started);
    assert!(app.exit.is_none());
    app.tick(started + END_LINGER + Duration::from_millis(10));
    assert_eq!(app.exit, Some(Exit::Quit));
    // A purged agent also counts as ended.
    let mut app = App::new(
        String::from("a"),
        Mode {
            exit_on_end: true,
            ..Mode::default()
        },
    );
    app.update(Update::Purged);
    app.tick(Instant::now() + END_LINGER + Duration::from_millis(10));
    assert_eq!(app.exit, Some(Exit::Quit));
}

#[test]
fn the_linger_follows_the_mode() {
    let mode = Mode {
        exit_on_end: true,
        linger: Duration::ZERO,
        ..Mode::default()
    };
    let mut app = App::new(String::from("a"), mode);
    app.update(Update::Purged);
    app.tick(Instant::now());
    assert_eq!(app.exit, Some(Exit::Quit));
}

#[test]
fn a_refused_first_attach_ends_the_view_with_the_daemon_error() {
    let mut app = App::new(String::from("missing"), Mode::default());
    let reply = json!({"error": {"code": "unknown_agent", "message": "Unknown agent."}});
    app.update(Update::Refused(reply.clone()));
    assert_eq!(app.exit, Some(Exit::Refused(reply)));
    // Before a snapshot, an unreachable daemon is shown on the connecting screen.
    let mut app = App::new(String::from("a"), Mode::default());
    app.update(Update::Disconnected);
    assert!(
        app.status.contains("not answering") && app.screen(true, Rect::new(0, 0, 80, 24)).is_none()
    );
}
